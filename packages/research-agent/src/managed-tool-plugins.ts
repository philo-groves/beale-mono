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
    id: "beale-investigations", name: "Investigations",
    description: "Use for investigation records, questions, observations, and configured experiments.",
    tools: ["investigation.candidates", "investigation.assign", "investigation.status", "investigation.recall", "investigation.question", "investigation.experiment", "investigation.observe", "investigation.next_action", "investigation.review_claim", "investigation.consolidate", "investigation.review_consolidation", "experiment.run"],
  },
  {
    id: "beale-runbooks", name: "Runbooks",
    description: "Use for reusable procedure documents, their revisions, and recorded executions.",
    tools: ["runbook.list", "runbook.get", "runbook.create", "runbook.append", "runbook.configure", "runbook.run"],
  },
  {
    id: "beale-reporting", name: "Reporting",
    description: "Use for report documents, revisions, and structured summaries of supported results.",
    tools: ["report.list", "report.get", "report.create", "report.revise", "synthesis.compose"],
  },
] as const;

export type ManagedToolPluginId = typeof MANAGED_TOOL_PLUGINS[number]["id"];
export const MANAGED_TOOL_PLUGIN_IDS: readonly ManagedToolPluginId[] = MANAGED_TOOL_PLUGINS.map((plugin) => plugin.id);

export const CORE_TOOL_NAMES = ["file.read", "file.write", "file.edit", "shell.run", "session.disposition"] as const;

/** New host tools must explicitly join a plugin or the small core surface. */
export function assertManagedToolOwnership(tools: readonly ResearchExecutableTool[]): void {
  for (const { descriptor } of tools) {
    if (descriptor.metadata?.provider === "mcp") continue;
    if (CORE_TOOL_NAMES.some((name) => name === descriptor.name)) continue;
    if (!managedToolPluginId(descriptor.name)) throw new Error(`Assign host tool ${descriptor.name} to a managed plugin before exposing it.`);
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

/** Context-only state, scoped to one agent; loading grants no new host capability. */
export class ManagedToolPluginSession {
  private readonly loaded = new Set<ManagedToolPluginId>();
  constructor(readonly options: readonly ManagedToolPluginOption[], restored: readonly string[] = []) {
    for (const id of restored) if (this.available(id)) this.loaded.add(id as ManagedToolPluginId);
  }

  private available(id: string): boolean {
    return this.options.some((option) => option.id === id && option.enabled && option.availableToolCount > 0);
  }

  snapshot(): ManagedToolPluginId[] { return [...this.loaded].sort(); }

  visible(toolName: string): boolean {
    const id = managedToolPluginId(toolName);
    return id === undefined || this.loaded.has(id);
  }

  createLoader(): ResearchExecutableTool {
    const description = [
      "Load an available plugin's tool schemas into this agent's context before calling its tools. Loading is idempotent and preserves existing host policy.",
      formatManagedToolPluginCatalog(this.options),
    ].join("\n");
    const parameters = { type: "object", required: ["plugins"], additionalProperties: false, properties: {
      plugins: { type: "array", minItems: 1, maxItems: MANAGED_TOOL_PLUGINS.length, uniqueItems: true, items: { type: "string", enum: MANAGED_TOOL_PLUGIN_IDS } },
    } };
    return {
      descriptor: { name: "plugins.load", transportName: "plugins_load", description, actionClasses: ["recall"], sideEffects: "none", requiredPermissions: [], inputSchema: parameters },
      parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
      execute: async (action) => {
        const startedAt = nowIso();
        const requested = action.input.plugins;
        if (!Array.isArray(requested) || requested.length === 0 || requested.length > MANAGED_TOOL_PLUGINS.length || requested.some((id) => typeof id !== "string" || !this.available(id))) {
          return { action, status: "blocked", startedAt, completedAt: nowIso(), summary: "Choose enabled plugins with available tools from the catalog.", followUpActions: [] };
        }
        for (const id of requested) this.loaded.add(id as ManagedToolPluginId);
        return { action, status: "complete", startedAt, completedAt: nowIso(), summary: "Plugin tools are available for the next model turn.", output: { loadedPluginIds: this.snapshot() }, followUpActions: [] };
      },
    };
  }
}
