import { existsSync, readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { PluginManagerWorkspace } from '../src/renderer/features/plugins/PluginManagerWorkspace';

describe('plugin manager workspace', () => {
  it('uses the shared centered regular-weight loading state', () => {
    const html = renderToStaticMarkup(createElement(PluginManagerWorkspace, {
      state: null,
      loading: true,
      busy: false,
      error: null,
      repositoryUrl: '',
      onRepositoryUrlChange: () => undefined,
      onAddFilesystem: () => undefined,
      onAddRepository: () => undefined,
      onSetEnabled: () => undefined,
      onRemove: () => undefined
    }));

    expect(html).toContain('class="centered-loading-state"');
    expect(html).toContain('class="centered-loading-state-spinner"');
    expect(html).toContain('<span>Loading plugins…</span>');
    expect(html).not.toContain('<strong>Loading plugins');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const loadingStyles = styles.match(/\.centered-loading-state\s*\{([^}]*)\}/u)?.[1] ?? '';
    expect(loadingStyles).toContain('place-content: center');
    expect(loadingStyles).toContain('font-weight: 400');
  });

  it('renders its Settings-style heading, install controls, and flat catalog in main content', () => {
    const html = renderToStaticMarkup(createElement(PluginManagerWorkspace, {
      state: null,
      loading: false,
      busy: false,
      error: null,
      repositoryUrl: '',
      onRepositoryUrlChange: () => undefined,
      onAddFilesystem: () => undefined,
      onAddRepository: () => undefined,
      onSetEnabled: () => undefined,
      onRemove: () => undefined
    }));

    expect(html).toContain('Add from Filesystem');
    expect(html).toContain('Add Repository');
    expect(html).toContain('<h1>Plugins</h1>');
    expect(html).toContain('Manage the plugins available to Beale agents.');
    expect(html.indexOf('Add from Filesystem')).toBeLessThan(html.indexOf('<h1>Plugins</h1>'));
    expect(html).toContain('No plugins installed');
    expect(html).not.toContain('<h2>0 Plugins</h2>');
  });

  it('shows built-in plugins as compact status rows', () => {
    const html = renderToStaticMarkup(createElement(PluginManagerWorkspace, {
      state: {
        registryPath: 'C:\\plugins.json',
        pluginStorePath: 'C:\\plugins',
        specVersion: '1.0.0',
        plugins: [{
          id: 'beale-introspection-builtin',
          name: 'beale-introspection',
          version: '0.1.0',
          description: 'Built-in tools.',
          enabled: true,
          status: 'ready',
          source: { kind: 'builtin', path: 'C:\\plugins\\beale-introspection' },
          installedAt: '2026-08-17T00:00:00.000Z',
          updatedAt: '2026-08-17T00:00:00.000Z',
          skills: [],
          mcpServers: [{ name: 'beale', transport: 'stdio', command: 'node', url: null, valid: true, errors: [] }],
          warnings: [],
          errors: []
        }]
      },
      loading: false,
      busy: false,
      error: null,
      repositoryUrl: '',
      onRepositoryUrlChange: () => undefined,
      onAddFilesystem: () => undefined,
      onAddRepository: () => undefined,
      onSetEnabled: () => undefined,
      onRemove: () => undefined
    }));

    expect(html).toContain('<strong>beale-introspection</strong>');
    expect(html).toContain('<small title="Enabled · 0.1.0">Enabled · 0.1.0</small>');
    expect(html).toContain('<span>Disable</span>');
    expect(html).not.toContain('<span>Remove</span>');
  });

  it('keeps plugin navigation out of the modal layer and marks it active in the sidebar', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const modalSource = readFileSync(new URL('../src/renderer/app/AppModals.tsx', import.meta.url), 'utf8');
    const sidebarSource = readFileSync(new URL('../src/renderer/features/workspaces/WorkspaceSidebar.tsx', import.meta.url), 'utf8');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');

    expect(appSource).toContain('<PluginManagerWorkspace');
    expect(modalSource).not.toContain('PluginManager');
    expect(sidebarSource).toContain("sidebar-utility-button${pluginsActive && !workspaceCreationActive ? ' active' : ''}");
    expect(styles).toMatch(/\.plugin-manager-body\s*\{[^}]*max-width:\s*var\(--session-content-max-width\);[^}]*margin-inline:\s*auto;/s);
    expect(styles).toMatch(/\.plugin-manager-body > \.resource-workspace-heading\s*\{[^}]*padding-top:\s*4px;/s);
    expect(styles).toMatch(/\.plugin-manager-list\s*\{[^}]*border-radius:\s*26px;[^}]*background:\s*var\(--panel-raised\);[^}]*padding:\s*3px 14px;/s);
    expect(styles).toMatch(/\.plugin-manager-row\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\) auto;[^}]*padding:\s*10px 0;/s);
    expect(styles).toMatch(/\.plugin-manager-row \+ \.plugin-manager-row\s*\{[^}]*border-top:\s*1px solid var\(--line\);/s);
    expect(styles).toMatch(/\.plugin-manager-actions button\s*\{[^}]*background:\s*var\(--panel-strong\);/s);
    expect(existsSync(new URL('../src/renderer/features/plugins/PluginManagerModal.tsx', import.meta.url))).toBe(false);
  });
});
