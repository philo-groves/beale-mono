import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import {
  AgentPluginRegistry, CORE_TOOL_NAMES, MANAGED_TOOL_PLUGINS, MANAGED_TOOL_PLUGIN_IDS,
  ManagedToolPluginSession, assertManagedToolOwnership, createResearchToolRegistry,
  formatManagedToolPluginCatalog, managedToolPluginId, managedToolPluginOptions,
  parseManagedToolPluginIds,
} from "../packages/research-agent/dist/index.js";
import { executeHostedUtilityOperation } from "../packages/app-server-runtime/dist/sessionRuntime.js";

const stub = (name) => ({
  descriptor: { name, transportName: name.replaceAll(".", "_"), description: "Read a synthetic record.", actionClasses: ["recall"], sideEffects: "read", requiredPermissions: [], inputSchema: { type: "object" } },
  parameters: { type: "object" },
  execute: async (action) => ({ action, status: "complete", summary: "Synthetic record read.", startedAt: "2026-01-01T00:00:00.000Z", completedAt: "2026-01-01T00:00:00.000Z", followUpActions: [] }),
});
const loadAction = (plugins) => ({ id: "load-example", toolName: "plugins.load", actionClass: "recall", input: { plugins } });

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

test("all seven bundled tool plugins have unique ownership and compact matching manifests", async () => {
  assert.equal(MANAGED_TOOL_PLUGINS.length, 7);
  const names = MANAGED_TOOL_PLUGINS.flatMap((plugin) => plugin.tools);
  assert.equal(new Set(names).size, names.length);
  for (const plugin of MANAGED_TOOL_PLUGINS) {
    assert.match(plugin.description, /^Use for /);
    assert.ok(plugin.description.length < 160);
    const manifest = JSON.parse(await readFile(resolve("app-server/resources/agent-plugins", plugin.id, "plugin.json"), "utf8"));
    assert.equal(manifest.name, plugin.id);
    assert.equal(manifest.description, plugin.description);
    for (const name of plugin.tools) {
      assert.equal(managedToolPluginId(name), plugin.id);
      assert.equal(managedToolPluginId(name.replaceAll(".", "_")), plugin.id);
    }
  }
  for (const name of CORE_TOOL_NAMES) assert.equal(managedToolPluginId(name), undefined);
  assertManagedToolOwnership([...names, ...CORE_TOOL_NAMES].map(stub));
  assert.throws(() => assertManagedToolOwnership([stub("example.unassigned")]), /Assign host tool/);
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
  assert.ok(disabled.createLoader().parameters.properties.plugins.items.enum.length > 0);
  const unavailable = new ManagedToolPluginSession(managedToolPluginOptions([], MANAGED_TOOL_PLUGIN_IDS));
  assert.match(formatManagedToolPluginCatalog(unavailable.options), /Unavailable in this session configuration/);
  assert.equal((await unavailable.createLoader().execute(loadAction(["beale-knowledge"]))).status, "blocked");
  assert.deepEqual(parseManagedToolPluginIds("none"), []);
  assert.throws(() => parseManagedToolPluginIds("beale-unknown"), /Unknown/);
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
    const builtinPlugins = MANAGED_TOOL_PLUGIN_IDS.map((id) => ({ id: `${id}-builtin`, path: resolve("app-server/resources/agent-plugins", id), installedAt: "2026-01-01T00:00:00.000Z", enabledByDefault: true }));
    const registry = new AgentPluginRegistry(directory, { builtinPlugins });
    assert.equal(registry.getState().plugins.length, 7);
    assert.ok(registry.getState().plugins.every((plugin) => plugin.enabled && plugin.status === "ready"));
    assert.deepEqual(registry.getAppServerRuntime().managedPluginIds.sort(), [...MANAGED_TOOL_PLUGIN_IDS].sort());
    for (const id of MANAGED_TOOL_PLUGIN_IDS) registry.setEnabled(`${id}-builtin`, false);
    const reloaded = new AgentPluginRegistry(directory, { builtinPlugins });
    assert.deepEqual(reloaded.getAppServerRuntime().managedPluginIds, []);
    assert.deepEqual(reloaded.getAppServerRuntime().args, ["--managed-plugins", "none"]);
    reloaded.setEnabled("beale-knowledge-builtin", true);
    assert.deepEqual(reloaded.getAppServerRuntime().managedPluginIds, ["beale-knowledge"]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
