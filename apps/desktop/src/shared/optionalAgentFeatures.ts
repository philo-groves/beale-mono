export const OPTIONAL_AGENT_FEATURES = [
  { id: 'beale-source', name: 'Source', description: 'Repository search and source analysis.' },
  { id: 'beale-provenance', name: 'Provenance', description: 'Revision history and public advisory references.' },
  { id: 'beale-knowledge', name: 'Knowledge', description: 'Workspace history, knowledge memory, and local resources.' },
  { id: 'beale-claims', name: 'Claims', description: 'Leads, findings, and evidence-backed claim revisions.' },
  { id: 'beale-runbooks', name: 'Runbooks', description: 'Reusable procedures and recorded executions.' },
  { id: 'beale-reporting', name: 'Reporting', description: 'Report documents and supported result summaries.' },
  { id: 'beale-browser', name: 'Browser', description: 'Full Chrome DevTools Protocol control for compatible browsers.' },
  { id: 'beale-fleet', name: 'Fleet', description: 'Use for listing, cloning, starting, and stopping operator-configured local virtual machines.' },
  { id: 'beale-introspection', name: 'Introspection', description: 'Workspace and session tools used by Quick Chat and research sessions.' }
] as const;

const OPTIONAL_AGENT_FEATURE_PLUGIN_IDS = new Set<string>(
  OPTIONAL_AGENT_FEATURES.map((feature) => `${feature.id}-builtin`)
);

export function isOptionalAgentFeaturePlugin(plugin: { id: string; source: { kind: string } }): boolean {
  return plugin.source.kind === 'builtin' && OPTIONAL_AGENT_FEATURE_PLUGIN_IDS.has(plugin.id);
}
