import { readFileSync } from "node:fs";
import type { ResearchExecutableTool } from "./tool-registry.js";
import { nowIso } from "./ids.js";

/** First-party tool ownership. Storage and execution remain app-server services. */
export const MANAGED_TOOL_PLUGINS = [
  {
    id: "beale-source", name: "Source",
    description: "Use for source search, code navigation, and structured text analysis.",
    tools: ["repository.search", "workspace.search", "code.detect", "code.outline", "code.query", "code.node_context", "code.references", "code.call_candidates", "analysis.transform"],
  },
  {
    id: "beale-provenance", name: "Provenance",
    description: "Use for repository revision history, public advisory references, and source provenance.",
    tools: ["repository.history", "repository.fetch_history", "prior_art.search", "prior_art.fetch"],
  },
  {
    id: "beale-knowledge", name: "Knowledge",
    description: "Use for reusable memory, workspace history, artifacts, local inspection, and resource inventory.",
    tools: ["memory.get", "memory.save", "memory.correct", "memory.link", "history.search", "history.mark_duplicate", "history.undo_duplicate", "storage.list", "local.inspection", "resource.catalog"],
  },
  {
    id: "beale-claims", name: "Claims",
    description: "Use for leads, findings, their evidence, and canonical claim revisions.",
    tools: ["claim.get", "lead.list", "lead.create", "finding.list", "finding.revise", "finding.transition", "finding.completion_check"],
  },
  {
    id: "beale-runbooks", name: "Runbooks",
    description: "Use for reusable procedure documents, their revisions, and recorded executions.",
    tools: ["runbook.list", "runbook.get", "runbook.prepare", "runbook.create", "runbook.append", "runbook.edit", "runbook.configure", "runbook.run", "experiment.run"],
  },
  {
    id: "beale-reporting", name: "Reporting",
    description: "Use for report documents, revisions, and structured summaries of supported results.",
    tools: ["report.list", "report.get", "report.create", "report.revise", "synthesis.compose"],
  },
  {
    id: "beale-browser", name: "Browser",
    description: "Use for controlling isolated Beale browser contexts or compatible external browsers through Chrome DevTools Protocol targets, commands, and events.",
    tools: ["browser.contexts", "browser.context.create", "browser.context.close", "browser.context.open", "browser.targets", "browser.connect", "browser.command", "browser.events", "browser.disconnect"],
  },
  {
    id: "beale-fleet", name: "Fleet",
    description: "Use for listing, cloning, starting, and stopping operator-configured local virtual machines.",
    tools: ["fleet.list", "fleet.clone", "fleet.start", "fleet.stop"],
  },
] as const;

export type ManagedToolPluginId = typeof MANAGED_TOOL_PLUGINS[number]["id"];
export const MANAGED_TOOL_PLUGIN_IDS: readonly ManagedToolPluginId[] = MANAGED_TOOL_PLUGINS.map((plugin) => plugin.id);
export const INTROSPECTION_HARNESS_FEATURE_ID = "beale-introspection-builtin";

export function isHarnessFeatureId(value: unknown): boolean {
  return isManagedToolPluginId(value) || value === INTROSPECTION_HARNESS_FEATURE_ID;
}

export interface PluginSkillResourceCounts {
  scripts: number;
  references: number;
  assets: number;
}

export interface ResearchPluginCatalogEntry {
  id: string;
  name: string;
  description?: string;
  toolCount?: number;
  skills: readonly { id: string; name: string; useWhen: string; path: string; resourceCounts?: PluginSkillResourceCounts }[];
  mcpServers: readonly string[];
}

export interface PluginPreviewOutput {
  pluginId: string;
  loaded: boolean;
  tools: string[];
  skills: { id: string; useWhen: string; resourceCounts?: PluginSkillResourceCounts }[];
}

export function decodeResearchPluginCatalog(value: unknown): ResearchPluginCatalogEntry[] {
  if (!Array.isArray(value) || value.length > 256) throw new Error("Plugin catalog must be an array of at most 256 plugins.");
  return value.map((item: unknown) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) throw new Error("Invalid plugin catalog entry.");
    const plugin = item as Record<string, unknown>;
    if (typeof plugin.id !== "string" || typeof plugin.name !== "string" || !Array.isArray(plugin.skills) || !Array.isArray(plugin.mcpServers)) {
      throw new Error("Invalid plugin catalog entry.");
    }
    if (plugin.skills.length > 1_000 || !plugin.mcpServers.every((server: unknown) => typeof server === "string")) {
      throw new Error("Invalid plugin catalog resources.");
    }
    if (plugin.toolCount !== undefined && (!Number.isSafeInteger(plugin.toolCount) || (plugin.toolCount as number) < 0)) {
      throw new Error("Invalid plugin tool count.");
    }
    const skills = plugin.skills.map((item: unknown) => {
      if (!item || typeof item !== "object" || Array.isArray(item)) throw new Error("Invalid plugin skill.");
      const skill = item as Record<string, unknown>;
      if (typeof skill.id !== "string" || typeof skill.name !== "string" || typeof skill.useWhen !== "string" || typeof skill.path !== "string") {
        throw new Error("Invalid plugin skill.");
      }
      const counts = skill.resourceCounts;
      if (counts !== undefined && (!counts || typeof counts !== "object" || Array.isArray(counts)
        || !(["scripts", "references", "assets"] as const).every((kind) => {
          const count = (counts as Record<string, unknown>)[kind];
          return Number.isSafeInteger(count) && (count as number) >= 0;
        }))) throw new Error("Invalid plugin skill resource counts.");
      return { id: skill.id, name: skill.name, useWhen: skill.useWhen, path: skill.path,
        ...(counts ? { resourceCounts: counts as PluginSkillResourceCounts } : {}) };
    });
    return { id: plugin.id, name: plugin.name, ...(typeof plugin.description === "string" ? { description: plugin.description } : {}),
      ...(typeof plugin.toolCount === "number" ? { toolCount: plugin.toolCount } : {}), skills, mcpServers: plugin.mcpServers as string[] };
  });
}

export function formatResearchPluginCatalog(plugins: readonly ResearchPluginCatalogEntry[]): string {
  const traditionalPlugins = plugins.filter((plugin) => !isHarnessFeatureId(plugin.id));
  if (traditionalPlugins.length === 0) return "No external plugins are available in this session.";
  return [
    "External Plugins (use plugins.preview for tool and skill summaries; plugins.load returns skill instructions and makes tool schemas available):",
    ...traditionalPlugins.map((plugin) =>
      `- ${plugin.id} (plugin; ${plugin.toolCount ?? (plugin.mcpServers.length > 0 ? "?" : 0)} ${plugin.toolCount === 1 ? "tool" : "tools"}, ${plugin.skills.length} ${plugin.skills.length === 1 ? "skill" : "skills"}):${plugin.description ? ` ${plugin.description.replace(/\s+/gu, " ").trim().slice(0, 240)}` : ""}`),
  ].join("\n");
}

export function formatHarnessFeatureCatalog(features: readonly ResearchPluginCatalogEntry[]): string {
  const harnessFeatures = features.filter((feature) => isHarnessFeatureId(feature.id));
  if (harnessFeatures.length === 0) return "";
  return [
    "Internal features (use features.preview for tool names; features.load makes their tool schemas available):",
    ...harnessFeatures.map((feature) =>
      `- ${feature.id} (feature; ${feature.toolCount ?? 0} ${feature.toolCount === 1 ? "tool" : "tools"}):${feature.description ? ` ${feature.description.replace(/\s+/gu, " ").trim().slice(0, 240)}` : ""}`),
  ].join("\n");
}

function boundedSkillUseWhen(skills: ResearchPluginCatalogEntry["skills"]): string[] {
  const descriptions = skills.map((skill) => [...skill.useWhen.replace(/\s+/gu, " ").trim()]);
  const lengths = descriptions.map((description) => description.length);
  const limit = 12_000;
  let low = 0;
  let high = lengths.reduce((maximum, length) => Math.max(maximum, length), 0);
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (lengths.reduce((sum, length) => sum + Math.min(length, middle), 0) <= limit) low = middle;
    else high = middle - 1;
  }
  const allocations = lengths.map((length) => Math.min(length, low));
  let remaining = limit - allocations.reduce((sum, length) => sum + length, 0);
  for (let index = 0; index < allocations.length && remaining > 0; index += 1) {
    if (allocations[index]! < lengths[index]!) {
      allocations[index]! += 1;
      remaining -= 1;
    }
  }
  return descriptions.map((description, index) => description.slice(0, allocations[index]!).join(""));
}

export const CORE_TOOL_NAMES = ["file.read", "file.write", "file.edit", "shell.run", "session.disposition", "tool_result.page"] as const;

/** New host tools must explicitly join a harness feature or the small core surface. */
export function assertManagedToolOwnership(tools: readonly ResearchExecutableTool[]): void {
  for (const { descriptor } of tools) {
    if (descriptor.metadata?.provider === "mcp") continue;
    if (CORE_TOOL_NAMES.some((name) => name === descriptor.name)) continue;
    if (!managedToolPluginId(descriptor.name)) throw new Error(`Assign host tool ${descriptor.name} to a harness feature before exposing it.`);
  }
}

export function isManagedToolPluginId(value: unknown): value is ManagedToolPluginId {
  return typeof value === "string" && MANAGED_TOOL_PLUGIN_IDS.some((id) => id === value);
}

export function managedToolPluginId(toolName: string): ManagedToolPluginId | undefined {
  return MANAGED_TOOL_PLUGINS.find((plugin) => plugin.tools.some((name) => name === toolName || name.replaceAll(".", "_") === toolName))?.id;
}

export function parseManagedToolPluginIds(value: string): ManagedToolPluginId[] {
  if (value === "none") return [];
  const ids = value.split(",");
  if (!ids.every(isManagedToolPluginId)) throw new Error("Unknown managed tool plugin ID.");
  return [...new Set(ids)];
}

export interface ManagedToolPluginOption {
  id: ManagedToolPluginId;
  name: string;
  description: string;
  enabled: boolean;
  availableToolCount: number;
}

export function formatManagedToolPluginCatalog(options: readonly ManagedToolPluginOption[]): string {
  return options.map((option) => `${option.id}: ${option.description}${!option.enabled ? " Disabled by operator." : option.availableToolCount === 0 ? " Unavailable in this session configuration." : ""}`).join("\n");
}

export function managedToolPluginOptions(tools: readonly ResearchExecutableTool[], enabledIds: readonly string[]): ManagedToolPluginOption[] {
  return MANAGED_TOOL_PLUGINS.map(({ id, name, description }) => ({
    id, name, description,
    enabled: enabledIds.includes(id),
    availableToolCount: enabledIds.includes(id) ? tools.filter((tool) => managedToolPluginId(tool.descriptor.name) === id).length : 0,
  }));
}

export function managedToolPluginCatalog(options: readonly ManagedToolPluginOption[]): ResearchPluginCatalogEntry[] {
  return options.filter((option) => option.enabled && option.availableToolCount > 0)
    .map((option) => ({ id: option.id, name: option.name, description: option.description, toolCount: option.availableToolCount, skills: [], mcpServers: [] }));
}

export function mergeResearchPluginCatalog(
  catalog: readonly ResearchPluginCatalogEntry[] | undefined,
  managed: readonly ManagedToolPluginOption[] | undefined,
  tools?: readonly ResearchExecutableTool[],
): ResearchPluginCatalogEntry[] {
  const entries = (catalog ?? []).filter((entry) => {
    if (!isManagedToolPluginId(entry.id)) return true;
    const option = managed?.find((candidate) => candidate.id === entry.id);
    return option?.enabled === true && (option.availableToolCount > 0 || entry.skills.length > 0 || entry.mcpServers.length > 0);
  });
  for (const plugin of managedToolPluginCatalog(managed ?? [])) {
    if (!entries.some((entry) => entry.id === plugin.id)) entries.push(plugin);
  }
  return entries.map((entry) => {
    const managedCount = managed?.find((option) => option.id === entry.id)?.availableToolCount ?? 0;
    if (!tools) return managedCount > 0 ? { ...entry, toolCount: managedCount } : entry;
    const mcpNames = new Set(tools.filter((tool) => entry.mcpServers.some((server) => tool.descriptor.name.startsWith(`mcp.${server}.`)))
      .map((tool) => tool.descriptor.name));
    return { ...entry, toolCount: managedCount + mcpNames.size };
  });
}

/** Context-only state, scoped to one agent; loading grants no new host capability. */
export class ManagedToolPluginSession {
  private readonly loaded = new Set<string>();
  constructor(readonly options: readonly ManagedToolPluginOption[], restored: readonly string[] = [], readonly catalog: readonly ResearchPluginCatalogEntry[] = []) {
    for (const id of restored) if (this.available(id)) this.loaded.add(id);
  }

  private available(id: string): boolean {
    if (isManagedToolPluginId(id)) {
      const option = this.options.find((candidate) => candidate.id === id);
      return option?.enabled === true && (option.availableToolCount > 0 || this.catalog.some((plugin) => plugin.id === id && (plugin.skills.length > 0 || plugin.mcpServers.length > 0)));
    }
    return this.catalog.some((plugin) => plugin.id === id);
  }

  snapshot(): string[] { return [...this.loaded].sort(); }

  visible(toolName: string): boolean {
    const id = managedToolPluginId(toolName);
    if (id) return this.loaded.has(id);
    const plugin = this.catalog.find((entry) => entry.mcpServers.some((server) => toolName.startsWith(`mcp.${server}.`)));
    return !plugin || this.loaded.has(plugin.id);
  }

  private availableIds(kind: "all" | "features" | "plugins" = "all"): string[] {
    return [...new Set([...this.options.filter((option) => this.available(option.id)).map((option) => option.id),
      ...this.catalog.filter((plugin) => this.available(plugin.id)).map((plugin) => plugin.id)])]
      .filter((id) => kind === "all" || (isHarnessFeatureId(id) === (kind === "features")));
  }

  createControlTools(tools: readonly ResearchExecutableTool[]): ResearchExecutableTool[] {
    return (["features", "plugins"] as const).flatMap((kind) => this.availableIds(kind).length > 0
      ? [this.createPreviewer(tools, kind), this.createLoader(kind)] : []);
  }

  createPreviewer(tools: readonly ResearchExecutableTool[], kind: "all" | "features" | "plugins" = "all"): ResearchExecutableTool {
    const ids = this.availableIds(kind);
    const feature = kind === "features";
    const field = feature ? "feature" : "plugin";
    const parameters = { type: "object", required: [field], additionalProperties: false, properties: {
      [field]: { type: "string", enum: ids },
    } };
    return {
      descriptor: { name: feature ? "features.preview" : "plugins.preview", transportName: feature ? "features_preview" : "plugins_preview",
        description: feature ? "Preview one available harness feature's tool names without loading its schemas." : "Preview one available plugin's tool names and skill use cases without loading its schemas or instructions.",
        actionClasses: ["recall"], sideEffects: "none", requiredPermissions: [], inputSchema: parameters },
      parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
      execute: async (action) => {
        const startedAt = nowIso();
        const id = action.input[field];
        if (typeof id !== "string" || !ids.includes(id)) {
          return { action, status: "blocked", startedAt, completedAt: nowIso(), summary: `Choose an available ${feature ? "harness feature" : "plugin"} from the catalog.`, followUpActions: [] };
        }
        const plugin = this.catalog.find((entry) => entry.id === id);
        const names = tools.filter((tool) => managedToolPluginId(tool.descriptor.name) === id
          || plugin?.mcpServers.some((server) => tool.descriptor.name.startsWith(`mcp.${server}.`)))
          .map((tool) => tool.descriptor.transportName ?? tool.descriptor.name);
        const boundedUseWhen = boundedSkillUseWhen(plugin?.skills ?? []);
        const output: PluginPreviewOutput = { pluginId: id, loaded: this.loaded.has(id), tools: [...new Set(names)].sort(),
          skills: (plugin?.skills ?? []).map((skill, index) => ({ id: skill.id,
            useWhen: boundedUseWhen[index]!, ...(skill.resourceCounts ? { resourceCounts: skill.resourceCounts } : {}) })) };
        return { action, status: "complete", startedAt, completedAt: nowIso(), summary: feature ? "Harness feature preview available." : "Plugin preview available.", output: feature ? { featureId: output.pluginId, loaded: output.loaded, tools: output.tools } : output, followUpActions: [] };
      },
    };
  }

  createLoader(kind: "all" | "features" | "plugins" = "all"): ResearchExecutableTool {
    const feature = kind === "features";
    const description = [
      feature ? "Load a harness feature's tool schemas into this agent's context before calling its tools. Loading preserves existing host policy." : "Load an available plugin's tool schemas into this agent's context before calling its tools. Loading is idempotent and preserves existing host policy.",
    ].join("\n");
    const ids = this.availableIds(kind);
    const field = feature ? "features" : "plugins";
    const parameters = { type: "object", required: [field], additionalProperties: false, properties: {
      [field]: { type: "array", minItems: 1, maxItems: ids.length, uniqueItems: true, items: { type: "string", enum: ids } },
    } };
    return {
      descriptor: { name: feature ? "features.load" : "plugins.load", transportName: feature ? "features_load" : "plugins_load", description, actionClasses: ["recall"], sideEffects: "none", requiredPermissions: [], inputSchema: parameters },
      parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
      execute: async (action) => {
        const startedAt = nowIso();
        const requested = action.input[field];
        if (!Array.isArray(requested) || requested.length === 0 || requested.length > ids.length || requested.some((id) => typeof id !== "string" || !ids.includes(id))) {
          return { action, status: "blocked", startedAt, completedAt: nowIso(), summary: `Choose enabled ${feature ? "harness features" : "plugins"} with available tools from the catalog.`, followUpActions: [] };
        }
        const skills = requested.flatMap((id) => this.catalog.find((plugin) => plugin.id === id)?.skills ?? [])
          .map((skill) => ({ id: skill.id, name: skill.name, instructions: readFileSync(skill.path, "utf8") }));
        for (const id of requested) this.loaded.add(id as string);
        return { action, status: "complete", startedAt, completedAt: nowIso(), summary: feature ? "Harness feature tools are available for the next model turn." : "Plugin resources are available for the next model turn.", output: feature ? { loadedFeatureIds: requested, skills } : { loadedPluginIds: this.snapshot(), skills }, followUpActions: [] };
      },
    };
  }
}
