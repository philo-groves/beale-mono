import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import {
  AgentPluginRegistry, CORE_TOOL_NAMES, MANAGED_TOOL_PLUGINS, MANAGED_TOOL_PLUGIN_IDS,
  ManagedToolPluginSession, assertManagedToolOwnership, createResearchToolRegistry, decodeResearchPluginCatalog,
  formatManagedToolPluginCatalog, formatHarnessFeatureCatalog, formatResearchPluginCatalog, managedToolPluginId, managedToolPluginOptions,
  mergeResearchPluginCatalog, parseManagedToolPluginIds,
} from "../packages/research-agent/dist/index.js";
import { executeHostedUtilityOperation } from "../packages/app-server-runtime/dist/sessionRuntime.js";

const stub = (name) => ({
  descriptor: { name, transportName: name.replaceAll(".", "_"), description: "Read a synthetic record.", actionClasses: ["recall"], sideEffects: "read", requiredPermissions: [], inputSchema: { type: "object" } },
  parameters: { type: "object" },
  execute: async (action) => ({ action, status: "complete", summary: "Synthetic record read.", startedAt: "2026-01-01T00:00:00.000Z", completedAt: "2026-01-01T00:00:00.000Z", followUpActions: [] }),
});
const loadAction = (plugins) => ({ id: "load-example", toolName: "plugins.load", actionClass: "recall", input: { plugins } });
const previewAction = (plugin) => ({ id: "preview-example", toolName: "plugins.preview", actionClass: "recall", input: { plugin } });

test("runtime assembly honors managed plugin selection while retaining core tools", async () => {
  const directory = await mkdtemp(join(tmpdir(), "beale-plugin-runtime-"));
  try {
    const args = ["tools", "list", "--workspace-root", directory, "--no-default-tool-config", "--file-read-root", directory, "--tool-family", "shell", "--tool-family", "repository-search"];
    const enabled = await executeHostedUtilityOperation("tools.list", args);
    const disabled = await executeHostedUtilityOperation("tools.list", [...args, "--managed-plugins", "none"]);
    const knowledge = await executeHostedUtilityOperation("tools.list", [...args, "--managed-plugins", "beale-knowledge"]);
    const names = (capture) => capture.tools.map((tool) => tool.name);
    for (const name of CORE_TOOL_NAMES) assert.ok(names(disabled).includes(name), name);
    assert.ok(names(enabled).includes("repository.search"));
    assert.ok(names(enabled).includes("prior_art.fetch"));
    assert.ok(names(enabled).includes("repository.fetch_history"));
    assert.ok(names(enabled).includes("memory.get"));
    assert.ok(names(enabled).includes("claim.get"));
    assert.ok(names(enabled).includes("browser.command"));
    assert.equal(names(disabled).includes("browser.command"), false);
    assert.equal(names(knowledge).includes("browser.command"), false);
    assert.ok(names(knowledge).includes("memory.get"));
    assert.equal(names(knowledge).includes("claim.get"), false);
    assert.equal(names(knowledge).includes("repository.search"), false);
    assert.equal(names(knowledge).includes("prior_art.fetch"), false);
    assert.equal(names(knowledge).includes("repository.fetch_history"), false);
    assert.ok(names(disabled).every((name) => CORE_TOOL_NAMES.includes(name)));
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("all seven internal optional features have unique tool ownership and compatibility manifests", async () => {
  assert.equal(MANAGED_TOOL_PLUGINS.length, 7);
  const names = MANAGED_TOOL_PLUGINS.flatMap((plugin) => plugin.tools);
  assert.equal(new Set(names).size, names.length);
  for (const plugin of MANAGED_TOOL_PLUGINS) {
    assert.match(plugin.description, /^Use for /);
    assert.ok(plugin.description.length < 160);
    const manifest = JSON.parse(await readFile(resolve("app-server/resources/harness-features", plugin.id, "plugin.json"), "utf8"));
    assert.equal(manifest.name, plugin.id);
    assert.equal(manifest.description, plugin.description);
    for (const name of plugin.tools) {
      assert.equal(managedToolPluginId(name), plugin.id);
      assert.equal(managedToolPluginId(name.replaceAll(".", "_")), plugin.id);
    }
  }
  for (const name of CORE_TOOL_NAMES) assert.equal(managedToolPluginId(name), undefined);
  assertManagedToolOwnership([...names, ...CORE_TOOL_NAMES].map(stub));
  assert.throws(() => assertManagedToolOwnership([stub("example.unassigned")]), /Assign host tool .* to a harness feature/);
  const external = stub("mcp.example.lookup");
  external.descriptor.metadata = { provider: "mcp" };
  assert.doesNotThrow(() => assertManagedToolOwnership([external]));
});

test("plugin catalog stays complete; loading is atomic, isolated, and bounded", async () => {
  const tools = [stub("memory.get"), stub("repository.search"), stub("file.read")];
  const options = managedToolPluginOptions(tools, ["beale-knowledge"]);
  assert.equal(options.length, 7);
  const catalog = formatManagedToolPluginCatalog(options);
  assert.equal(catalog.split("\n").length, 7);
  assert.match(catalog, /beale-source: .*Disabled by operator/);
  const session = new ManagedToolPluginSession(options);
  const other = new ManagedToolPluginSession(options);
  const registry = createResearchToolRegistry([session.createLoader()]);
  assert.ok(JSON.stringify(session.createLoader().descriptor).length < 2500, "initial discovery schema must remain compact");
  const preview = await session.createPreviewer(tools).execute(previewAction("beale-knowledge"));
  assert.deepEqual(preview.output, { pluginId: "beale-knowledge", loaded: false, tools: ["memory_get"], skills: [] });
  assert.deepEqual(session.snapshot(), []);
  assert.equal(session.visible("file_read"), true);
  assert.equal(session.visible("memory_get"), false);
  assert.equal((await registry.execute(loadAction(["beale-knowledge", "beale-source"]))).result.status, "blocked");
  assert.deepEqual(session.snapshot(), []);
  assert.equal((await registry.execute(loadAction(["beale-knowledge"]))).result.status, "complete");
  assert.equal(session.visible("memory_get"), true);
  assert.equal(other.visible("memory_get"), false);
  await registry.execute(loadAction(["beale-knowledge"]));
  assert.deepEqual(session.snapshot(), ["beale-knowledge"]);
  const disabled = new ManagedToolPluginSession(managedToolPluginOptions(tools, []), session.snapshot());
  assert.deepEqual(disabled.snapshot(), []);
  assert.deepEqual(disabled.createLoader().parameters.properties.plugins.items.enum, []);
  const unavailable = new ManagedToolPluginSession(managedToolPluginOptions([], MANAGED_TOOL_PLUGIN_IDS));
  assert.match(formatManagedToolPluginCatalog(unavailable.options), /Unavailable in this session configuration/);
  assert.equal((await unavailable.createLoader().execute(loadAction(["beale-knowledge"]))).status, "blocked");
  assert.deepEqual(parseManagedToolPluginIds("none"), []);
  assert.throws(() => parseManagedToolPluginIds("beale-unknown"), /Unknown/);
});

test("prompt catalog lists plugins only; preview fairly bounds skill use cases", async () => {
  const plugins = [
    { id: "example-a", name: "Example A", mcpServers: [], skills: [
      { id: "skill-a", name: "Skill A", useWhen: "A".repeat(8_000), path: "example-a.md", resourceCounts: { scripts: 1, references: 2, assets: 0 } },
      { id: "skill-b", name: "Skill B", useWhen: "B".repeat(8_000), path: "example-b.md", resourceCounts: { scripts: 0, references: 0, assets: 1 } },
    ] },
    { id: "example-b", name: "Example B", mcpServers: [], skills: [
      { id: "skill-c", name: "Skill C", useWhen: "C".repeat(2_000), path: "example-c.md", resourceCounts: { scripts: 0, references: 0, assets: 0 } },
    ] },
  ];
  const prompt = formatResearchPluginCatalog(plugins);
  assert.match(prompt, /- example-a \(plugin; 0 tools, 2 skills\):/);
  assert.doesNotMatch(prompt, /skill-a|skill-b|skill-c|A{20}|B{20}|C{20}|Example A|Skill A/);
  const session = new ManagedToolPluginSession([], [], [{ ...plugins[0], skills: [...plugins[0].skills, plugins[1].skills[0]] }]);
  const result = await session.createPreviewer([]).execute(previewAction("example-a"));
  assert.equal(result.status, "complete");
  assert.deepEqual(result.output.skills.map((skill) => skill.useWhen.length), [5_000, 5_000, 2_000]);
  assert.deepEqual(result.output.skills[0].resourceCounts, { scripts: 1, references: 2, assets: 0 });
  assert.deepEqual(session.snapshot(), []);
});

test("plugin catalog reports tools available in this session and skill counts", () => {
  const tools = [stub("memory.get"), stub("mcp.example.tools.inspect"), stub("mcp.example.tools.read")];
  const managed = managedToolPluginOptions(tools, ["beale-knowledge"]);
  const catalog = [
    { id: "beale-knowledge", name: "Knowledge", skills: [], mcpServers: [] },
    { id: "example-plugin", name: "Example Plugin", skills: [
      { id: "example-skill", name: "Example Skill", useWhen: "Inspect a synthetic example.", path: "example.md",
        resourceCounts: { scripts: 2, references: 1, assets: 0 } },
    ], mcpServers: ["example.tools"] },
  ];
  const result = mergeResearchPluginCatalog(catalog, managed, tools);
  assert.match(formatHarnessFeatureCatalog(result), /beale-knowledge \(feature; 1 tool\):/);
  assert.doesNotMatch(formatResearchPluginCatalog(result), /beale-knowledge/);
  assert.match(formatResearchPluginCatalog(result), /example-plugin \(plugin; 2 tools, 1 skill\):/);
  assert.match(formatResearchPluginCatalog(catalog), /example-plugin \(plugin; \? tools, 1 skill\):/);
});

test("optional feature controls and traditional plugin controls accept only their own catalog entries", async () => {
  const tools = [stub("memory.get"), stub("mcp.example.tools.read")];
  const managed = managedToolPluginOptions(tools, ["beale-knowledge"]);
  const catalog = [{ id: "example-plugin", name: "Example Plugin", skills: [], mcpServers: ["example.tools"] }];
  const session = new ManagedToolPluginSession(managed, [], catalog);
  const controls = session.createControlTools(tools);
  assert.deepEqual(controls.map((tool) => tool.descriptor.name), ["features.preview", "features.load", "plugins.preview", "plugins.load"]);
  const featurePreviewer = controls.find((tool) => tool.descriptor.name === "features.preview");
  const featureLoader = controls.find((tool) => tool.descriptor.name === "features.load");
  const pluginLoader = controls.find((tool) => tool.descriptor.name === "plugins.load");
  const featurePreview = await featurePreviewer.execute({ id: "preview-feature", toolName: "features.preview", actionClass: "recall", input: { feature: "beale-knowledge" } });
  assert.deepEqual(featurePreview.output, { featureId: "beale-knowledge", loaded: false, tools: ["memory_get"] });
  assert.equal((await featureLoader.execute({ id: "load-plugin-as-feature", toolName: "features.load", actionClass: "recall", input: { features: ["example-plugin"] } })).status, "blocked");
  assert.equal((await pluginLoader.execute(loadAction(["beale-knowledge"]))).status, "blocked");
  assert.equal((await featureLoader.execute({ id: "load-feature", toolName: "features.load", actionClass: "recall", input: { features: ["beale-knowledge"] } })).status, "complete");
  assert.equal(session.visible("memory.get"), true);
  assert.equal(session.visible("mcp.example.tools.read"), false);
  assert.equal((await pluginLoader.execute(loadAction(["example-plugin"]))).status, "complete");
  assert.equal(session.visible("mcp.example.tools.read"), true);
});

test("Introspection is presented and loaded as a harness feature", async () => {
  const catalog = [{ id: "beale-introspection-builtin", name: "beale-introspection", description: "Workspace and session tools.", skills: [], mcpServers: ["beale-introspection.beale"], toolCount: 1 }];
  const tool = stub("mcp.beale-introspection.beale.workspace_list");
  const session = new ManagedToolPluginSession([], [], catalog);
  const controls = session.createControlTools([tool]);
  assert.deepEqual(controls.map(({ descriptor }) => descriptor.name), ["features.preview", "features.load"]);
  assert.match(formatHarnessFeatureCatalog(catalog), /Internal features/);
  assert.doesNotMatch(formatResearchPluginCatalog(catalog), /beale-introspection/);
  assert.equal(session.visible(tool.descriptor.name), false);
  const preview = await controls[0].execute({ id: "preview-introspection", toolName: "features.preview", actionClass: "recall", input: { feature: catalog[0].id } });
  assert.deepEqual(preview.output, { featureId: catalog[0].id, loaded: false, tools: [tool.descriptor.transportName] });
  const load = await controls[1].execute({ id: "load-introspection", toolName: "features.load", actionClass: "recall", input: { features: [catalog[0].id] } });
  assert.equal(load.status, "complete");
  assert.equal(session.visible(tool.descriptor.name), true);
});

test("plugin skill resource counts include nested files in standard resource directories", async () => {
  const directory = await mkdtemp(join(tmpdir(), "beale-skill-resources-example-"));
  try {
    const pluginRoot = join(directory, "example-plugin");
    const skillRoot = join(pluginRoot, "skills", "example-skill");
    for (const path of ["scripts/nested", "references", "assets/icons"]) {
      await mkdir(join(skillRoot, path), { recursive: true });
    }
    await writeFile(join(pluginRoot, "plugin.json"), JSON.stringify({
      $schema: "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json", name: "example-plugin",
    }));
    await writeFile(join(skillRoot, "SKILL.md"), "---\nname: Example Skill\ndescription: Inspect synthetic records.\n---\n");
    for (const path of ["scripts/run.ts", "scripts/nested/check.ts", "references/guide.md", "assets/icons/icon.svg", "assets/data.json"]) {
      await writeFile(join(skillRoot, path), "Synthetic fixture.\n");
    }
    const registry = new AgentPluginRegistry(join(directory, "registry"), { builtinPlugins: [] });
    const state = registry.addFromFilesystem(pluginRoot);
    assert.deepEqual(state.plugins[0].skills[0].resourceCounts, { scripts: 2, references: 1, assets: 2 });
    const runtime = registry.getAppServerRuntime();
    const catalog = decodeResearchPluginCatalog(JSON.parse(await readFile(runtime.pluginCatalogPath, "utf8")));
    assert.deepEqual(catalog[0].skills[0].resourceCounts, { scripts: 2, references: 1, assets: 2 });
    assert.doesNotMatch(formatResearchPluginCatalog(catalog), /example-skill|Inspect synthetic records/);
    const preview = await new ManagedToolPluginSession([], [], catalog).createPreviewer([]).execute(previewAction(catalog[0].id));
    assert.equal(preview.status, "complete");
    assert.deepEqual(preview.output.skills, [{ id: "example-skill", useWhen: "Inspect synthetic records.",
      resourceCounts: { scripts: 2, references: 1, assets: 2 } }]);
    assert.throws(() => decodeResearchPluginCatalog([{ ...catalog[0], skills: [{ ...catalog[0].skills[0],
      resourceCounts: { scripts: -1, references: 1, assets: 2 } }] }]), /Invalid plugin skill resource counts/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("MCP-only discovery supplies exact bundled plugin counts without workspace storage", async () => {
  const directory = await mkdtemp(join(tmpdir(), "beale-plugin-counts-example-"));
  try {
    const registry = new AgentPluginRegistry(directory, { builtinPlugins: [{ id: "beale-introspection-builtin", path: resolve("app-server/resources/harness-features/beale-introspection"), installedAt: "2026-01-01T00:00:00.000Z", enabledByDefault: true }], runtimeEnvironment: () => ({
      BEALE_INTROSPECTION_URL: "http://127.0.0.1:12345", BEALE_INTROSPECTION_TOKEN: "example-token",
    }) });
    registry.addFromFilesystem(resolve("managed-plugins/apple-security-devices"));
    const runtime = registry.getAppServerRuntime();
    const capture = await executeHostedUtilityOperation("tools.list", [
      "tools", "list", "--mcp-only", "--mcp-config", runtime.mcpConfigPath,
      ...runtime.allowedMcpServers.flatMap((server) => ["--allow-mcp-server", server]),
    ]);
    const catalog = mergeResearchPluginCatalog(runtime.pluginCatalog, [], capture.tools.map(({ name }) => stub(name)));
    const formatted = formatResearchPluginCatalog(catalog);
    assert.match(formatted, /\(plugin; 19 tools, 1 skill\): Realistic Apple security-research environments/);
    assert.match(formatHarnessFeatureCatalog(catalog), /\(feature; 12 tools\): Default Beale tools/);
    assert.doesNotMatch(formatted, /beale-introspection/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("external plugin skill bodies and MCP tools appear only after explicit load", async () => {
  const directory = await mkdtemp(join(tmpdir(), "beale-plugin-load-example-"));
  try {
    const path = join(directory, "SKILL.md");
    await writeFile(path, "Use synthetic evidence for this example.");
    const session = new ManagedToolPluginSession([], [], [{
      id: "example-plugin", name: "Example Plugin", mcpServers: ["example-plugin.tools"],
      skills: [{ id: "example-skill", name: "Example Skill", useWhen: "Inspect a synthetic example.", path }],
    }]);
    assert.equal(session.visible("mcp.example-plugin.tools.inspect"), false);
    const preview = await session.createPreviewer([stub("mcp.example-plugin.tools.inspect")]).execute(previewAction("example-plugin"));
    assert.equal(preview.status, "complete");
    assert.deepEqual(preview.output, { pluginId: "example-plugin", loaded: false, tools: ["mcp_example-plugin_tools_inspect"],
      skills: [{ id: "example-skill", useWhen: "Inspect a synthetic example." }] });
    assert.deepEqual(session.snapshot(), []);
    assert.equal(session.visible("mcp.example-plugin.tools.inspect"), false);
    assert.equal((await session.createPreviewer([]).execute(previewAction("unknown-plugin"))).status, "blocked");
    const result = await session.createLoader().execute(loadAction(["example-plugin"]));
    assert.equal(result.status, "complete");
    assert.deepEqual(result.output.skills, [{ id: "example-skill", name: "Example Skill", instructions: "Use synthetic evidence for this example." }]);
    assert.equal(session.visible("mcp.example-plugin.tools.inspect"), true);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("unavailable built-in plugins are absent from the model catalog", async () => {
  const catalog = [
    { id: "beale-source", name: "Source", skills: [], mcpServers: [] },
    { id: "example-plugin", name: "Example Plugin", skills: [], mcpServers: [] },
  ];
  const options = managedToolPluginOptions([], MANAGED_TOOL_PLUGIN_IDS);
  assert.deepEqual(mergeResearchPluginCatalog(catalog, options).map((plugin) => plugin.id), ["example-plugin"]);
  const session = new ManagedToolPluginSession(options, [], catalog);
  assert.deepEqual(session.createLoader().parameters.properties.plugins.items.enum, ["example-plugin"]);
  assert.equal((await session.createLoader().execute(loadAction(["beale-source"]))).status, "blocked");
});

test("registry forks retain validation hooks when adding plugin discovery", async () => {
  const options = managedToolPluginOptions([stub("memory.get")], MANAGED_TOOL_PLUGIN_IDS);
  const tool = stub("memory.get");
  tool.descriptor.validationHooks = ["example-validation"];
  const registry = createResearchToolRegistry([tool], {
    managedPlugins: options,
    validationHooks: { "example-validation": () => "Synthetic validation rejection." },
  });
  const fork = registry.fork([new ManagedToolPluginSession(options).createLoader()]);
  assert.equal(registry.find("plugins.load"), undefined);
  assert.deepEqual(fork.managedPlugins, options);
  const result = await fork.execute({ id: "read-example", actionClass: "recall", toolName: "memory.get", input: {} });
  assert.equal(result.result.status, "blocked");
});

test("bundled plugin defaults and disablement persist in launch arguments", async () => {
  const directory = await mkdtemp(join(tmpdir(), "beale-plugin-catalog-"));
  try {
    const builtinPlugins = MANAGED_TOOL_PLUGIN_IDS.map((id) => ({ id: `${id}-builtin`, path: resolve("app-server/resources/harness-features", id), installedAt: "2026-01-01T00:00:00.000Z", enabledByDefault: true }));
    const registry = new AgentPluginRegistry(directory, { builtinPlugins });
    assert.equal(registry.getState().plugins.length, 7);
    assert.ok(registry.getState().plugins.every((plugin) => plugin.enabled && plugin.status === "ready"));
    assert.deepEqual(registry.getAppServerRuntime().managedPluginIds.sort(), [...MANAGED_TOOL_PLUGIN_IDS].sort());
    for (const id of MANAGED_TOOL_PLUGIN_IDS) registry.setEnabled(`${id}-builtin`, false);
    const reloaded = new AgentPluginRegistry(directory, { builtinPlugins });
    assert.deepEqual(reloaded.getAppServerRuntime().managedPluginIds, []);
    const disabledRuntime = reloaded.getAppServerRuntime();
    assert.deepEqual(disabledRuntime.args, ["--managed-plugins", "none", "--plugin-catalog", disabledRuntime.pluginCatalogPath]);
    assert.deepEqual(JSON.parse(await readFile(disabledRuntime.pluginCatalogPath, "utf8")), []);
    reloaded.setEnabled("beale-knowledge-builtin", true);
    assert.deepEqual(reloaded.getAppServerRuntime().managedPluginIds, ["beale-knowledge"]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("Introspection migration retains the saved enabled state and updates its builtin path", async () => {
  const directory = await mkdtemp(join(tmpdir(), "beale-introspection-migration-example-"));
  const id = "beale-introspection-builtin";
  const installedAt = "2026-01-01T00:00:00.000Z";
  try {
    const previous = new AgentPluginRegistry(directory, { builtinPlugins: [{ id, path: resolve("managed-plugins/beale-introspection"), installedAt }] });
    previous.setEnabled(id, false);
    const nextPath = resolve("app-server/resources/harness-features/beale-introspection");
    const current = new AgentPluginRegistry(directory, { builtinPlugins: [{ id, path: nextPath, installedAt }] });
    const feature = current.getState().plugins.find((plugin) => plugin.id === id);
    assert.equal(feature?.source.path, nextPath);
    assert.equal(feature?.enabled, false);
    assert.equal(feature?.status, "ready");
    assert.doesNotMatch(formatHarnessFeatureCatalog(current.getAppServerRuntime().pluginCatalog), /beale-introspection/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("retired Browser Use and Terminator builtins are removed from saved plugin settings", async () => {
  const directory = await mkdtemp(join(tmpdir(), "beale-retired-plugin-example-"));
  const registryPath = join(directory, "agent-plugins.json");
  try {
    const installedAt = "2026-01-01T00:00:00.000Z";
    const introspectionPath = resolve("app-server/resources/harness-features/beale-introspection");
    await writeFile(registryPath, JSON.stringify({ version: 1, plugins: [
      { id: "beale-browser-use-builtin", source: { kind: "builtin", path: join(directory, "retired-browser") }, enabled: true, installedAt },
      { id: "beale-terminator-builtin", source: { kind: "builtin", path: join(directory, "retired-computer-use") }, enabled: false, installedAt },
      { id: "beale-introspection-builtin", source: { kind: "builtin", path: introspectionPath }, enabled: false, installedAt },
    ] }));
    const registry = new AgentPluginRegistry(directory, { builtinPlugins: [{ id: "beale-introspection-builtin", path: introspectionPath, installedAt }] });
    assert.deepEqual(registry.getState().plugins.map((plugin) => [plugin.id, plugin.enabled]), [["beale-introspection-builtin", false]]);
    assert.deepEqual(JSON.parse(await readFile(registryPath, "utf8")).plugins.map((plugin) => [plugin.id, plugin.enabled]), [["beale-introspection-builtin", false]]);
    assert.deepEqual(registry.getAppServerRuntime().allowedMcpServers, []);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
