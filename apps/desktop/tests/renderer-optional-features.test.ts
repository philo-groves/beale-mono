import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { MANAGED_TOOL_PLUGIN_IDS } from '@beale/app-server-runtime/protocol';
import type { AgentPluginRecord, AgentPluginRegistryState } from '../src/shared/types';
import { OPTIONAL_AGENT_FEATURES } from '../src/shared/optionalAgentFeatures';
import { PluginManagerWorkspace } from '../src/renderer/features/plugins/PluginManagerWorkspace';
import { OptionalFeaturesSettingsView, SettingsSidebar, settingsSectionLabel } from '../src/renderer/features/settings/SettingsModal';

const featurePlugin: AgentPluginRecord = {
  id: 'beale-source-builtin', name: 'beale-source', version: '0.1.0', description: 'Source tools.',
  enabled: true, status: 'ready', source: { kind: 'builtin', path: '/example/agent-plugins/beale-source' },
  installedAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z',
  skills: [], mcpServers: [], warnings: [], errors: []
};
const traditionalPlugin: AgentPluginRecord = {
  ...featurePlugin,
  id: 'meta-skills-builtin', name: 'meta-skills', description: 'Research guidance.'
};
const introspectionFeature: AgentPluginRecord = {
  ...featurePlugin,
  id: 'beale-introspection-builtin', name: 'beale-introspection', description: 'Introspection tools.'
};
const state: AgentPluginRegistryState = {
  registryPath: '/example/agent-plugins.json', pluginStorePath: '/example/plugins', specVersion: '1.0.0',
  plugins: [featurePlugin, introspectionFeature, traditionalPlugin]
};

describe('optional agent features', () => {
  it('classifies the six internal app-server tool groups', () => {
    expect(OPTIONAL_AGENT_FEATURES.map((feature) => feature.id)).toEqual([...MANAGED_TOOL_PLUGIN_IDS, 'beale-introspection']);
  });

  it('places their toggles in Agent Settings and leaves traditional plugins in Plugins', () => {
    const sidebar = renderToStaticMarkup(createElement(SettingsSidebar, {
      collapsed: false, section: 'optional-features', error: null,
      onChangeSection: () => undefined, onResizePointerDown: () => undefined
    }));
    const settings = renderToStaticMarkup(createElement(OptionalFeaturesSettingsView, {
      pluginState: state, loading: false, busy: false, error: null, onSetEnabled: () => undefined
    }));
    const plugins = renderToStaticMarkup(createElement(PluginManagerWorkspace, {
      state, loading: false, busy: false, error: null, repositoryUrl: '',
      onRepositoryUrlChange: () => undefined, onAddFilesystem: () => undefined,
      onAddRepository: () => undefined, onSetEnabled: () => undefined, onRemove: () => undefined
    }));

    expect(settingsSectionLabel('optional-features')).toBe('Features');
    expect(sidebar).toContain('<span>Features</span>');
    expect(sidebar).not.toContain('Computer Permissions');
    expect(sidebar).not.toContain('Terminator');
    expect(sidebar).toContain('aria-current="page"');
    expect(settings).toContain('type="checkbox" aria-label="Enable Source" checked=""');
    expect(settings).toContain('type="checkbox" aria-label="Enable Introspection" checked=""');
    expect(settings).toContain('type="checkbox" aria-label="Enable Reporting" disabled=""');
    expect(plugins).not.toContain('<strong>beale-source</strong>');
    expect(plugins).not.toContain('<strong>beale-introspection</strong>');
    expect(plugins).toContain('<strong>meta-skills</strong>');
  });
});
