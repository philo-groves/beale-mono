import assert from "node:assert/strict";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  DEFAULT_SECURITY_RESEARCH_PROFILE,
  effectiveClaimClassifications,
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
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const options = { hasTools: true, hasMemoryTools: true, researchProfile: profile };
  const defaultPrompt = createResearchSystemPrompt(options);
  assert.equal(createResearchSystemPrompt({ ...options, promptTemplate: defaultResearchSystemPromptTemplate() }), defaultPrompt);
  const custom = createResearchSystemPrompt({ ...options, promptTemplate: "{{boundary}}\nProfile: {{profile.name}}\n{{identity}}" });
  assert.ok(custom.startsWith("Scope and authority:"));
  assert.ok(custom.includes("Profile: Security"));
  assert.ok(custom.includes(profile.agent.role));
  assert.match(defaultResearchSystemPromptTemplate(), /\{\{tools\}\}\n\n\{\{features\}\}\n\n\{\{plugins\}\}\n\n\{\{collaboration\}\}/);
  assert.match(defaultResearchSystemPromptTemplate(), /\{\{prompt_extensions\}\}/);
  assert.doesNotMatch(defaultResearchSystemPromptTemplate(), /\{\{goal\}\}/);
  assert.doesNotMatch(defaultResearchSystemPromptTemplate(), /\{\{reports\}\}/);
  const withPlugins = createResearchSystemPrompt({ ...options, pluginCatalog: [{
    id: "example-plugin", name: "Example Plugin", mcpServers: [],
    skills: [{ id: "example-skill", name: "Example Skill", useWhen: "Inspect synthetic data.", path: "example.md",
      resourceCounts: { scripts: 0, references: 1, assets: 2 } }],
  }] });
  assert.match(withPlugins, /External Plugins.*\n- example-plugin \(plugin; 0 tools, 1 skill\):/);
  assert.doesNotMatch(withPlugins, /example-skill|Inspect synthetic data/);
  assert.doesNotMatch(withPlugins, /Example Plugin|Example Skill/);
  const existingTemplate = createResearchSystemPrompt({ ...options, pluginCatalog: [{
    id: "example-plugin", name: "Example Plugin", mcpServers: [], skills: [],
  }], promptTemplate: "{{boundary}}\n{{identity}}" });
  assert.match(existingTemplate, /External Plugins[^\n]*\n- example-plugin \(plugin; 0 tools, 0 skills\):/);
  const existingFeatureTemplate = createResearchSystemPrompt({ ...options, pluginCatalog: [{
    id: "beale-knowledge", name: "Knowledge", description: "Inspect synthetic workspace history.", toolCount: 3,
    mcpServers: [], skills: [],
  }, {
    id: "beale-introspection-builtin", name: "Introspection", description: "Inspect synthetic workspaces and sessions.", toolCount: 2,
    mcpServers: ["beale-introspection.beale"], skills: [],
  }], promptTemplate: "{{boundary}}\n{{identity}}" });
  assert.match(existingFeatureTemplate, /Internal features[^\n]*\n- beale-knowledge \(feature; 3 tools\):/);
  assert.match(existingFeatureTemplate, /- beale-introspection-builtin \(feature; 2 tools\):/);
  assert.doesNotMatch(existingFeatureTemplate, /External Plugins/);
  assert.doesNotMatch(existingFeatureTemplate, /Optional features/);
  assert.throws(() => validateResearchSystemPromptTemplate("{{identity}}"), /boundary/);
  assert.throws(() => validateResearchSystemPromptTemplate("{{boundary}}\n{{unknown}}"), /Unknown prompt variable/);
});

test("default prompt uses plain guidance lines and bullets only for feature and plugin catalogs", () => {
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
  assert.ok(bulletLines.some((line) => line.startsWith("- beale-knowledge (feature;")));
  assert.ok(bulletLines.some((line) => line.startsWith("- sink (Sink):")));
  assert.ok(bulletLines.every((line) =>
    line.includes(" (plugin;") || line.includes(" (feature;")
    || profile.memory.types.some((type) => line.startsWith(`- ${type.id} (`))
  ));
  assert.match(prompt, /Scope and authority:\nThe host-supplied workspace context/);
  assert.match(prompt, /Tool routing:\nProfile-recognized material kinds/);
  assert.match(prompt, /Child surfaces are stable, concrete components or subcomponents/);
  assert.match(prompt, /Keep those in research notes and records/);
  assert.match(prompt, /Substantiated conclusions still need evidence/);
  assert.match(prompt, /New research may change that state, and completion does not restrict further work/);
  assert.match(prompt, /A progressBlocker records a concrete external dependency/);
  assert.match(prompt, /Prefer core APIs, SDKs, and trust boundaries over low-impact examples/);
  assert.match(prompt, /third party's application or account does not become authorized merely because it uses an in-scope vendor product/);
  assert.match(prompt, /discover covered official components through their provenance and recorded scope rules/);
  assert.match(prompt, /an independent review marked needs-work are work to pursue, not reasons to mark a finding Blocked/);
  assert.match(prompt, /Treat its gaps as the next research or review steps/);
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
  const session = createResearchSystemPrompt({ ...options, promptTemplate: "{{boundary}}\n{{session}}" });
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
  assert.match(session, /Treat existing records and prior transcript as historical state/);
  assert.match(memory, /Preserve materially useful new facts/);
  assert.match(tools, /Profile-recognized material kinds/);
  assert.match(boundary, /Host-verified same-Subject reference workspaces are read-only/);
  assert.match(toolsSection, /Tool routing:\n/);
  assert.match(toolsSection, /Resource first touch:\n/);
  assert.match(toolsSection, /Execution dependencies:\n/);
  assert.match(toolsSection, /Before calling a VM slot occupied or a running guest unrelated, check fleet\.list/);
  assert.match(toolsSection, /Shell fallback:\n/);
  assert.doesNotMatch(toolsSection, /cross-workspace mutation|At evidence checkpoints|After repairing an execution dependency/);
  assert.match(session, /At evidence checkpoints, rank at most three candidates/);
  assert.match(memory, /After repairing an execution dependency/);
  assert.match(boundary, /Never expose host credentials/);
});

test("goal guidance adds status signaling only when Goal mode is enabled", () => {
  const options = {
    hasTools: true,
    researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE,
    promptTemplate: "{{boundary}}\n{{session}}\n{{prompt_extensions}}",
  };
  const regular = createResearchSystemPrompt(options);
  const persistent = createResearchSystemPrompt({ ...options, goalEnabled: true, hasSessionDispositionTool: true });

  assert.match(regular, /Session progress:\nTreat existing records/);
  assert.match(regular, /At evidence checkpoints/);
  assert.doesNotMatch(regular, /Goal status:|session\.disposition|Persistent Goal mode:/);
  assert.match(persistent, /Persistent Goal mode:\nContinue researching/);
  assert.match(persistent, /continue its proof and independent review toward Verified/);
  assert.match(persistent, /Honor an explicit user request to stop at a specific lower finding maturity/);
  assert.match(persistent, /Goal status:\nBefore the root final response in Goal mode/);
  assert.match(persistent, /Verified is the default Goal stopping maturity/);
  assert.doesNotMatch(persistent, /\n{3,}/);
});

test("Goals feature text and catalog references require Goal Mode", () => {
  const catalog = [{ id: "beale-goals-builtin", name: "Goals", description: "Synthetic Goals guidance.",
    promptExtension: "Synthetic Goal Mode instructions.", mcpServers: [], skills: [] }];
  const options = { hasTools: true, pluginCatalog: catalog,
    promptTemplate: "{{boundary}}\n{{features}}\n{{session}}\nExtensions begin\n{{prompt_extensions}}\nExtensions end" };
  const regular = createResearchSystemPrompt(options);
  assert.doesNotMatch(regular, /beale-goals-builtin|Synthetic Goals guidance|Synthetic Goal Mode instructions|Persistent Goal mode|objective_achieved/u);
  assert.match(regular, /Session progress:/u);

  const persistent = createResearchSystemPrompt({ ...options, goalEnabled: true });
  assert.match(persistent, /beale-goals-builtin \(feature; 0 tools\): Synthetic Goals guidance\./u);
  assert.match(persistent, /Extensions begin\nGoals \(beale-goals-builtin\):\nSynthetic Goal Mode instructions\.\nExtensions end/u);
  assert.doesNotMatch(persistent, /Persistent Goal mode:/u);
  assert.equal(persistent.match(/Synthetic Goal Mode instructions\./gu)?.length, 1);

  const defaultGoal = createResearchSystemPrompt({ hasTools: true, goalEnabled: true,
    pluginCatalog: [{ id: "beale-goals-builtin", name: "Goals", mcpServers: [], skills: [] }] });
  assert.match(defaultGoal, /Goals \(beale-goals-builtin\):\nPersistent Goal mode:\nContinue researching the supplied objective/u);
  const disabledFeature = createResearchSystemPrompt({ ...options, goalEnabled: true, pluginCatalog: [] });
  assert.doesNotMatch(disabledFeature, /Synthetic Goal Mode instructions|Persistent Goal mode|objective_achieved/u);

  const legacyTemplate = createResearchSystemPrompt({ ...options, goalEnabled: true,
    promptTemplate: "{{boundary}}\n{{goal}}" });
  assert.match(legacyTemplate, /Goals \(beale-goals-builtin\):\nSynthetic Goal Mode instructions\./u);
  assert.equal(legacyTemplate.match(/Synthetic Goal Mode instructions\./gu)?.length, 1);
  assert.match(legacyTemplate, /Session progress:/u);
  const legacyWithExtensions = createResearchSystemPrompt({ ...options, goalEnabled: true,
    promptTemplate: "{{boundary}}\n{{goal}}\n{{prompt_extensions}}" });
  assert.equal(legacyWithExtensions.match(/Synthetic Goal Mode instructions\./gu)?.length, 1);
  const withoutCatalog = createResearchSystemPrompt({ hasTools: true, goalEnabled: true,
    promptTemplate: "{{boundary}}\n{{prompt_extensions}}" });
  assert.match(withoutCatalog, /Goals \(beale-goals-builtin\):\nPersistent Goal mode:/u);
});

test("discovery guidance treats existing findings as prior art while allowing requested follow-up", () => {
  const prompt = createResearchSystemPrompt({
    hasTools: true,
    researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE,
    pluginCatalog: pluginCatalog("beale-claims"),
    promptTemplate: "{{boundary}}\n{{session}}\n{{claims}}",
  });
  assert.match(prompt, /When the user asks to find a vulnerability, a severity, or a bug class, pursue a distinct new candidate/);
  assert.match(prompt, /Work on an existing claim when the user directly asks for that claim or asks to continue, reproduce, verify, or report existing work/);
  assert.match(prompt, /Before lead\.create or finding\.create, search and inspect plausible existing claims for the same root cause and affected condition/);
  assert.match(prompt, /sharing a classification or severity never makes two claims duplicates/);
});

test("collaboration guidance separates delegation, documentation, profile protocol, and runtime policy", () => {
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
  assert.match(prompt, /Research documentation:\nStore reusable research synthesis/);
  assert.match(prompt, /Profile collaboration protocol:\nKeep exploit claims/);
  assert.match(prompt, /Active collaboration settings:\nCollaboration mode is adaptive/);
  assert.ok(prompt.indexOf("Delegation:") < prompt.indexOf("Research documentation:"));
  assert.ok(prompt.indexOf("Research documentation:") < prompt.indexOf("Profile collaboration protocol:"));
  assert.ok(prompt.indexOf("Profile collaboration protocol:") < prompt.indexOf("Active collaboration settings:"));
  assert.doesNotMatch(prompt, /\n{3,}/);
  assert.doesNotMatch(withoutTools, /Delegation:|Research documentation:|Active collaboration settings:/);
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

test("the bundled Security profile separates claim classifications from knowledge memory", () => {
  const security = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);

  assert.equal(security.name, "Security");
  assert.equal(security.capabilities.reportsEnabled, true);
  assert.deepEqual(security.capabilities.defaultToolFamilies, ["shell", "repository-search", "file-read"]);
  assert.equal(Object.hasOwn(normalizeResearchProfile(generalResearchProfile()).capabilities, "reportsEnabled"), false);
  assert.equal(security.presentation.sessionHeatPalette, undefined);
  assert.ok(security.memory.types.every((type) => type.sessionHeat === undefined));
  assert.deepEqual(
    security.memory.types.filter((type) => type.lifecycle === "active").map((type) => type.id),
    ["invariant", "mitigation", "sink", "trajectory"],
  );
  assert.deepEqual(security.claims.classifications.map((classification) => classification.id),
    ["security.vulnerability"]);
});

test("stored security profiles hide retired split classifications from active guidance", () => {
  const stored = normalizeResearchProfile({
    ...DEFAULT_SECURITY_RESEARCH_PROFILE,
    agent: { ...DEFAULT_SECURITY_RESEARCH_PROFILE.agent, memoryInstructions: [
      ...DEFAULT_SECURITY_RESEARCH_PROFILE.agent.memoryInstructions,
      "Represent composition with componentClaimIds on security.chain and security.primitive findings.",
    ] },
    claims: { classifications: [
      ...DEFAULT_SECURITY_RESEARCH_PROFILE.claims.classifications,
      { id: "security.primitive", name: "Legacy isolated", pluralName: "Legacy isolated", description: "Legacy classification.", defaultProjection: "finding", order: 20 },
      { id: "security.chain", name: "Legacy composite", pluralName: "Legacy composite", description: "Legacy classification.", defaultProjection: "finding", composite: true, order: 30 },
    ] },
  });
  const originalHash = researchProfileHash(stored);
  assert.deepEqual(effectiveClaimClassifications(stored).map((classification) => classification.id), ["security.vulnerability"]);
  assert.equal(researchProfileHash(stored), originalHash);
  const prompt = createResearchSystemPrompt({ hasTools: true, pluginCatalog: pluginCatalog("beale-knowledge", "beale-claims"), researchProfile: stored, promptTemplate: "{{boundary}}\n{{memory}}\n{{claims}}" });
  assert.match(prompt, /security\.vulnerability/);
  assert.doesNotMatch(prompt, /security\.primitive|security\.chain|componentClaimIds/);
});

test("memory, claims, and runbook prompt sections follow enabled optional features", () => {
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

test("the bundled Security profile does not prescribe collaboration recipes", () => {
  const security = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  assert.deepEqual(security.collaboration.recipes, []);
});

test("stored Security profile guidance keeps unlisted resources eligible under broad scope", () => {
  const legacy = { ...DEFAULT_SECURITY_RESEARCH_PROFILE, workspace: {
    ...DEFAULT_SECURITY_RESEARCH_PROFILE.workspace,
    boundaryInstructions: [
      "Only explicitly in-scope assets are authorized; exclusions and constraints override research objectives.",
      "Ambient resources are non-authoring inventory. Require Auto-Reviewed relevance before their first substantive research touch; approval permits tracking and history work, not live-target authorization.",
    ],
  } };
  const prompt = createResearchSystemPrompt({ hasTools: true, researchProfile: legacy });
  assert.match(prompt, /An unlisted resource may be in scope/);
  assert.match(prompt, /Auto-Reviewed scope membership and relevance/);
  assert.doesNotMatch(prompt, /Only explicitly in-scope assets/);
});

test("retired bundled memory types stay out of model-facing catalogs", () => {
  const securityPrompt = createResearchSystemPrompt({
    hasTools: true,
    hasMemoryTools: true,
    hasRunbookTools: true,
    pluginCatalog: pluginCatalog("beale-knowledge", "beale-runbooks"),
    researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE,
  });
  assert.match(securityPrompt, /- sink \(Sink\)/);
  assert.doesNotMatch(securityPrompt, /- asset \(Asset\)/);
  assert.doesNotMatch(securityPrompt, /- source \(Source\)/);
  assert.doesNotMatch(securityPrompt, /- flow-endpoint \(Flow Endpoint\)/);
  assert.doesNotMatch(securityPrompt, /- bug \(Historical Bug\)/);
  assert.doesNotMatch(securityPrompt, /- procedure \(Procedure\)/);
  assert.match(securityPrompt, /Ambient resources are non-authoring inventory/);
  assert.match(securityPrompt, /Auto-Reviewed first touch/);
  assert.match(securityPrompt, /provenance and build identity, relevant advisories and fixes/);
  assert.match(securityPrompt, /does not expand that boundary/);
  assert.match(securityPrompt, /Execution dependencies:/);
  assert.match(securityPrompt, /lifecycle and ownership, route and address, listening service, host identity, then authentication/);
  assert.match(securityPrompt, /Do not substitute a host account for a guest account/);
  assert.match(securityPrompt, /prefer the known-good unprivileged mode/);
  assert.match(securityPrompt, /update its environment runbook/);
  assert.match(securityPrompt, /matching workflow runbook for reproduction-grade evidence/);
  assert.match(securityPrompt, /Put exploratory security tests, proof iterations, and reproduction attempts into the matching runbook/);
  assert.match(securityPrompt, /Do not defer runbook creation until a finding is ready to advance/);
  assert.match(securityPrompt, /setup, runtime, and cleanup.*feature tags/);
  assert.match(securityPrompt, /successful runIds.*phases they prove/);
  assert.match(securityPrompt, /workspace history search to recover prior work across sessions/);
});

test("the bundled Security profile offers four suggestion categories", () => {
  const security = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
  const securityLongshot = security.workflows.find((workflow) => workflow.id === "longshot");
  assert.equal(security.version, "1.17.0");
  assert.deepEqual(security.workflows.map((workflow) => workflow.id), ["discovery", "chaining", "reporting", "longshot"]);
  assert.ok(security.workflows.every((workflow) => workflow.goalSuggestionCount === 4));
  assert.equal(securityLongshot?.name, "Longshot");
  assert.equal(securityLongshot?.description, "Hunt for ambitious, reportable high- or critical-severity vulnerabilities.");
  assert.match(securityLongshot?.goalSuggestionInstructions.join(" ") ?? "", /broad attack surface.*explicit systemic impact ceiling/);
  assert.match(securityLongshot?.goalSuggestionInstructions.join(" ") ?? "", /not.*binary verification task/);
  assert.match(securityLongshot?.promptInstructions.join(" ") ?? "", /severity, and reportability evidence-gated/);
  assert.ok(security.collaboration.recipes.length === 0);
});

test("suggestion lanes do not constrain live collaboration guidance", () => {
  const securityPrompt = createResearchSystemPrompt({ hasTools: true, hasCollaborationTools: true, researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE, workflowId: "chaining" });
  assert.doesNotMatch(securityPrompt, /Exploit-chain cell|reachability-analyst|Active research workflow/);
  assert.doesNotMatch(securityPrompt, /Construction Explorer/);
});

test("Longshot remains a suggestion lane and is absent from live prompts", () => {
  const securityPrompt = createResearchSystemPrompt({ hasTools: true, hasCollaborationTools: true, researchProfile: DEFAULT_SECURITY_RESEARCH_PROFILE, workflowId: "longshot" });

  assert.doesNotMatch(securityPrompt, /Longshot|reportable high or critical impact|Security discovery cell/);
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
  const profile = normalizeResearchProfile(DEFAULT_SECURITY_RESEARCH_PROFILE);
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

test("the direct run boundary rejects non-security profiles", async () => {
  const profile = normalizeResearchProfile(generalResearchProfile());
  await assert.rejects(runResearchAgent({
    prompt: "Synthetic research prompt.",
    resolvedResearchProfile: { profile, hash: researchProfileHash(profile), source: "explicit" },
    executor: createDeterministicAgentExecutor(),
  }), /only Security research sessions/);
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

test("profile-selected skills remain inert until the host explicitly selects them", async () => {
  const workspaceRoot = join(tmpdir(), `app-server-profile-skill-${process.pid}-${Date.now()}`);
  try {
    await mkdir(workspaceRoot, { recursive: true });
    const input = structuredClone(DEFAULT_SECURITY_RESEARCH_PROFILE);
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
