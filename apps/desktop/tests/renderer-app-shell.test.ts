import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { FleetState } from '@beale/app-server-runtime/protocol';
import type { HostEnvironment, RunDetail, WorkspaceSnapshot } from '@shared/types';
import { AppHeaderTitle, StaticAppHeaderTitle, type AppHeaderViewIcon } from '../src/renderer/app/AppHeaderTitle';
import { AppNavigationRail, ProviderLimitIndicator, resolveAppNavigationDestination } from '../src/renderer/app/AppNavigationRail';
import { FleetSidebar } from '../src/renderer/features/fleet/FleetSidebar';
import { FleetSettingsView } from '../src/renderer/features/settings/FleetSettingsView';
import { SettingsSidebar } from '../src/renderer/features/settings/SettingsModal';
import { headerMenuInlineEnd, rightmostHeaderMenuControl, TopBar } from '../src/renderer/app/TopBar';
import {
  activeRunDetailForSelection,
  appShellClassName,
  selectedRunStatus,
  shouldShowHeaderResearchControls,
  windowControlPlatformForState
} from '../src/renderer/view-models/appShell';

describe('renderer app shell view model', () => {
  it('matches header icons to workspace and sidenav destinations', () => {
    const workspaceHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'ExampleCo iOS Parser',
      workspaceViewTitle: 'Memory',
      detail: null,
    }));
    expect(workspaceHeader).toContain('lucide-folder');
    expect(workspaceHeader).toContain('aria-label="ExampleCo iOS Parser, Memory"');
    expect(workspaceHeader).toContain('title="ExampleCo iOS Parser"');
    expect(workspaceHeader).toContain('title="Memory"><span>Memory</span>');

    const newResearchHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'Parser',
      workspaceViewTitle: 'New Research',
      detail: { run: { id: 'run_old', title: 'Old Session', promptMarkdown: 'Old prompt' } } as RunDetail,
    }));
    expect(newResearchHeader).toContain('aria-label="Parser, New Research"');
    expect(newResearchHeader).not.toContain('Old Session');
    expect(newResearchHeader).not.toContain('Old Topic');

    const sessionHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'Parser',
      workspaceViewTitle: null,
      detail: { run: { id: 'run_current', title: 'Inspect parser states', promptMarkdown: 'Inspect parser states' } } as RunDetail
    }));
    expect(sessionHeader).toContain('class="app-header-session-title app-header-static-title"');
    expect(sessionHeader).toContain('title="Inspect parser states"');
    expect(sessionHeader).not.toContain('<button');
    const focusedHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'Parser',
      detail: { run: { id: 'run_current', title: 'Inspect parser states', promptMarkdown: '' } },
      surfaceFocus: { name: 'Request parser', additionalCount: 2 }
    }));
    expect(focusedHeader).toContain('aria-label="Parser, Inspect parser states, Request parser +2"');
    expect(focusedHeader).toContain('title="Request parser +2"');
    expect(focusedHeader).not.toContain('app-header-surface-separator');
    expect(focusedHeader).toContain('class="app-header-surface-name">Request parser</span>');
    expect(focusedHeader).toContain('class="app-header-surface-count">+2</span>');
    expect(focusedHeader).not.toContain('<button');
    const singleSurfaceHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'Parser', detail: { run: { id: 'run_current', title: 'Inspect parser states', promptMarkdown: '' } },
      surfaceFocus: { name: 'Request parser', additionalCount: 0 }
    }));
    expect(singleSurfaceHeader).toContain('title="Request parser"');
    expect(singleSurfaceHeader).not.toContain('app-header-surface-count');
    expect(newResearchHeader).not.toContain('app-header-surface-title');
    const mainTitle = 'class="app-header-workspace-title app-header-static-title"';
    for (const header of [workspaceHeader, newResearchHeader, sessionHeader]) {
      expect(header.indexOf('class="app-header-divider"')).toBeLessThan(header.indexOf(mainTitle));
      expect(header.match(/class="app-header-divider"/gu)).toHaveLength(1);
    }

    const viewIcons: Array<[AppHeaderViewIcon, string]> = [
      ['settings', 'lucide-settings'],
      ['settings-appearance', 'lucide-palette'],
      ['settings-profiles', 'lucide-user-round-cog'],
      ['settings-providers', 'lucide-server-cog'],
      ['settings-ticketing', 'lucide-ticket'],
      ['automations', 'lucide-calendar-clock'],
      ['reporting', 'lucide-file-text'],
      ['plugins', 'lucide-plug'],
      ['fleet', 'lucide-server']
    ];
    for (const [icon, iconClass] of viewIcons) {
      const header = renderToStaticMarkup(createElement(StaticAppHeaderTitle, {
        primaryTitle: 'View',
        secondaryTitle: 'Detail',
        icon
      }));
      expect(header).toContain(iconClass);
      expect(header.indexOf('class="app-header-divider"')).toBeLessThan(header.indexOf(mainTitle));
      expect(header.match(/class="app-header-divider"/gu)).toHaveLength(1);
    }
  });

  it('renders Fleet before Plugins and reserves the bottom for limits', () => {
    const onNavigate = () => undefined;
    const html = renderToStaticMarkup(createElement(AppNavigationRail, {
      active: 'settings', onOpenHome: onNavigate, onOpenAutomations: onNavigate,
      onOpenPlugins: onNavigate, onOpenSettings: onNavigate, onOpenFleet: onNavigate
    }));

    expect(html).toContain('aria-label="Main navigation"');
    for (const label of ['Home', 'Automations', 'Plugins', 'Agent Settings', 'Fleet']) {
      expect(html).toContain(`aria-label="${label}"`);
      expect(html).toContain(`data-tooltip="${label}"`);
    }
    expect(html.indexOf('aria-label="Home"')).toBeLessThan(html.indexOf('aria-label="Automations"'));
    expect(html.indexOf('aria-label="Fleet"')).toBeLessThan(html.indexOf('aria-label="Plugins"'));
    expect(html.indexOf('aria-label="Plugins"')).toBeLessThan(html.indexOf('aria-label="Agent Settings"'));
    expect(html.indexOf('aria-label="Agent Settings"')).toBeLessThan(html.indexOf('class="app-navigation-rail-bottom"'));
    expect(html).toContain('class="app-navigation-rail-filled-icon"');
    expect(html).toContain('aria-current="page"');
    expect(html).toContain('width="18"');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
    const railStyles = styles.match(/\.app-navigation-rail\s*\{([^}]*)\}/u)?.[1] ?? '';
    const topStyles = styles.match(/\.app-navigation-rail-top,\s*\.app-navigation-rail-sections\s*\{([^}]*)\}/u)?.[1] ?? '';
    expect(styles).toContain('--navigation-rail-width: 50px');
    expect(railStyles).toContain('padding: 4px');
    expect(railStyles).toContain('background: transparent');
    expect(styles).not.toMatch(/\.app-shell\.sidebar-collapsed \.app-navigation-rail\s*\{[^}]*width:/u);
    expect(topStyles).toContain('gap: 6px');
    expect(styles).toContain('.app-navigation-rail-top {\n  margin-top: 8px;');
    expect(styles).toContain('.app-navigation-rail-sections {\n  margin-top: 0;');
    expect(styles).toMatch(/\.app-navigation-rail-button::after,\s*\.header-icon-tooltip::after\s*\{[^}]*content: attr\(data-tooltip\);/s);
    expect(styles).toMatch(/\.app-navigation-rail-button::after\s*\{[^}]*left: calc\(100% \+ 10px\);/s);
    expect(styles).toContain('.app-navigation-rail-button:focus-visible::after');
  });

  it('nests Session inside Weekly on a provider rail icon and shows the last update', () => {
    const html = renderToStaticMarkup(createElement(ProviderLimitIndicator, {
      limit: {
        providerId: 'openai-codex', checkedAt: '2033-01-01T00:00:00Z', status: 'available', detail: null,
        windows: [
          { id: 'session', label: 'Session Limit', usedPercent: 25, resetsAt: '2033-01-01T05:00:00Z', durationMinutes: 300 },
          { id: 'weekly', label: 'Weekly Limit', usedPercent: 70, resetsAt: '2033-01-07T00:00:00Z', durationMinutes: 10_080 }
        ]
      }
    }));
    expect(html).toContain('role="tooltip"');
    expect(html).toContain('class="app-navigation-limit-icon has-inner"');
    expect(html).toContain('viewBox="0 0 36 36"');
    expect(html).toContain('height="15"');
    expect(html).toMatch(/data-limit="Weekly"[^>]*stroke-dasharray="30 100"|stroke-dasharray="30 100"[^>]*data-limit="Weekly"/u);
    expect(html).toMatch(/data-limit="Session"[^>]*stroke-dasharray="75 100"|stroke-dasharray="75 100"[^>]*data-limit="Session"/u);
    expect(html).toContain('>OpenAI</div>');
    expect(html).toContain('>Session</span>');
    expect(html).toContain('>Weekly</span>');
    expect(html).toContain('75% left |');
    expect(html).toContain('30% left |');
    expect(html).toContain('Last Update');
    expect(html).not.toContain('role="progressbar"');

    const single = renderToStaticMarkup(createElement(ProviderLimitIndicator, {
      limit: {
        providerId: 'xai', checkedAt: '2033-01-01T00:00:00Z', status: 'available', detail: null,
        windows: [{ id: 'weekly', label: 'Weekly Limit', usedPercent: 40, resetsAt: null, durationMinutes: 10_080 }]
      }
    }));
    expect(single).not.toContain('has-inner');
    expect(single).toContain('data-limit="Weekly"');
    expect(single).toContain('60% left');
  });

  it('lists Fleet resources in the sidebar and opens machine details inline', () => {
    const onNavigate = () => undefined;
    const fleetState: FleetState = {
      machineId: 'machine-local', role: 'primary', enabled: true, available: true, error: null,
      machines: [{
        id: 'vm-example', name: 'Example VM', backend: 'tart', state: 'stopped', base: false,
        canRun: true, canClone: false, guestPlatform: 'linux', privilege: 'standard', sshConfigured: false,
        sshIdentityConfigured: false, sshKnownHostsConfigured: false, sshHost: null, sshUser: null, owner: null
      }],
      remoteMachines: [],
      appServers: [{ id: 'server-example', name: 'Example Server', url: 'https://server.example.test' }],
      browsers: [{ id: 'browser-example', name: 'Example Browser', kind: 'external', enabled: true }],
      browserHomePage: 'https://duckduckgo.com/',
      workspaceTargets: {}, primary: null, requiredWorkspaceIds: [], optionalWorkspaceIds: [], lastMachineByWorkspace: {}
    };
    const settings = renderToStaticMarkup(createElement(SettingsSidebar, {
      collapsed: false, section: 'general', error: null,
      onChangeSection: onNavigate, onResizePointerDown: onNavigate
    }));
    const fleet = renderToStaticMarkup(createElement(FleetSidebar, {
      collapsed: false, state: fleetState, selection: { section: 'app-servers', itemId: null }, error: null,
      statuses: {
        local: { id: 'local', state: 'online', message: 'Connected.' },
        'server-example': { id: 'server-example', state: 'issue', message: 'Connection needs attention.' }
      },
      onSelect: onNavigate, onSetBrowserEnabled: async () => undefined, onResizePointerDown: onNavigate
    }));
    expect(settings).not.toContain('>Fleet</span>');
    expect(fleet).toContain('sidebar-wordmark">Fleet</div>');
    expect(fleet).not.toContain('aria-label="Fleet" class="workspace-item"');
    expect(fleet.indexOf('aria-label="App Servers"')).toBeLessThan(fleet.indexOf('aria-label="Browsers"'));
    expect(fleet.indexOf('aria-label="Browsers"')).toBeLessThan(fleet.indexOf('aria-label="Virtual Machines"'));
    expect(fleet.match(/<svg /gu)).toHaveLength(3);
    expect(fleet).not.toContain('fleet-sidebar-item-marker');
    expect(fleet).toContain('sidebar-server-status-dot online');
    expect(fleet).toContain('aria-label="Local, Connected."');
    expect(fleet).toContain('sidebar-server-status-dot issue');
    expect(fleet).toMatch(/Example Server<\/span><span class="sidebar-server-status-dot issue"/u);
    expect(fleet).toContain('aria-label="Example Server, Connection needs attention."');
    for (const label of ['Example Server', 'Example Browser', 'Example VM']) expect(fleet).toContain(label);
    expect(fleet).toContain('aria-label="Enable Example Browser"');
    expect(fleet).toContain('type="checkbox"');
    expect(fleet).toContain('checked=""');
    expect(fleet).toContain('aria-label="Fleet sections"');

    const viewProps = {
      state: fleetState, loadError: null, onSelect: onNavigate,
      onStateChange: onNavigate, onRefresh: async () => undefined,
      remoteAccessSettings: null, remoteAccessBusy: false,
      onDetectRemoteAccess: async () => undefined,
      onSaveRemoteAccess: async () => undefined
    };
    const overview = renderToStaticMarkup(createElement(FleetSettingsView, {
      ...viewProps, selection: { section: 'app-servers', itemId: null }
    }));
    expect(overview).not.toContain('Enable Fleet');
    expect(overview).not.toContain('Fleet role');
    expect(overview).not.toContain('This Beale instance');
    expect(overview).toContain('<strong>Local</strong>');
    const machine = renderToStaticMarkup(createElement(FleetSettingsView, {
      ...viewProps, selection: { section: 'machines', itemId: 'vm-example' }
    }));
    expect(machine).toContain('aria-label="Configure Example VM"');
    expect(machine).toContain('Save VM');
    expect(machine).not.toContain('role="dialog"');
    expect(machine).not.toContain('modal-backdrop');
    const server = renderToStaticMarkup(createElement(FleetSettingsView, {
      ...viewProps, selection: { section: 'app-servers', itemId: 'server-example' }
    }));
    expect(server).toContain('aria-label="Configure Example Server"');
    expect(server).toContain('Show QR Code');
    expect(server).toMatch(/Test Connection<\/button><button[^>]*>.*?Show QR Code<\/button>/u);
    expect(server).not.toContain('role="dialog"');
    const localServer = renderToStaticMarkup(createElement(FleetSettingsView, {
      ...viewProps, selection: { section: 'app-servers', itemId: 'local' }
    }));
    expect(localServer).toContain('aria-label="Local"');
    expect(localServer).toContain('<h2>Local</h2>');
    expect(localServer).toContain('settings-form-squircle fleet-detail-form fleet-text-form');
    expect(localServer).toContain('Test Connection');
    expect(localServer).toContain('Show QR Code');
    expect(localServer).toMatch(/Test Connection<\/button><button[^>]*>.*?Show QR Code<\/button>/u);
    expect(localServer).toContain('Restart App Server');
    const browsers = renderToStaticMarkup(createElement(FleetSettingsView, {
      ...viewProps, selection: { section: 'browsers', itemId: null }
    }));
    expect(browsers).not.toContain('type="checkbox"');
    expect(browsers).toContain('Default page');
    expect(browsers).toContain('https://duckduckgo.com/');
  });

  it('fills only the active navigation icon', () => {
    const onNavigate = () => undefined;
    for (const active of ['home', 'automations', 'plugins', 'settings', 'fleet'] as const) {
      const html = renderToStaticMarkup(createElement(AppNavigationRail, {
        active, onOpenHome: onNavigate, onOpenAutomations: onNavigate,
        onOpenPlugins: onNavigate, onOpenSettings: onNavigate, onOpenFleet: onNavigate
      }));
      expect(html.match(/class="app-navigation-rail-filled-icon"/gu)).toHaveLength(1);
      expect(html.match(/class="lucide lucide-/gu)).toHaveLength(4);
      expect(html.match(/aria-current="page"/gu)).toHaveLength(1);
      if (active === 'fleet') {
        expect(html).toContain('<rect x="2" y="2" width="20" height="8" rx="2"></rect>');
        expect(html).toContain('<rect x="2" y="14" width="20" height="8" rx="2"></rect>');
      } else {
        expect(html).toContain('lucide-server');
      }
    }
  });

  it('keeps a navigation destination active as sessions open', () => {
    const base = { settingsOpen: false, fleetOpen: false, automationsOpen: false, pluginsOpen: false };
    expect(resolveAppNavigationDestination(base)).toBe('home');
    expect(resolveAppNavigationDestination({ ...base, automationsOpen: true })).toBe('automations');
    expect(resolveAppNavigationDestination({ ...base, pluginsOpen: true })).toBe('plugins');
    expect(resolveAppNavigationDestination({ ...base, settingsOpen: true })).toBe('settings');
    expect(resolveAppNavigationDestination({ ...base, fleetOpen: true })).toBe('fleet');

    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    expect(appSource).toContain('resolveAppNavigationDestination({ settingsOpen, fleetOpen, automationsOpen, pluginsOpen })');
    const onNavigate = () => undefined;
    const html = renderToStaticMarkup(createElement(AppNavigationRail, {
      active: resolveAppNavigationDestination(base),
      onOpenHome: onNavigate, onOpenAutomations: onNavigate,
      onOpenPlugins: onNavigate, onOpenSettings: onNavigate, onOpenFleet: onNavigate
    }));
    expect(html).toContain('class="app-navigation-rail-button active" data-tooltip="Home"');
    expect(html.match(/aria-current="page"/gu)).toHaveLength(1);
  });

  it('rounds the outer edges of the secondary sidebar and main content', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const sidebarStyles = styles.match(/(?:^|\n)\.sidebar\s*\{([^}]*)\}/u)?.[1] ?? '';
    const workbenchStyles = styles.match(/(?:^|\n)\.workbench\s*\{([^}]*)\}/u)?.[1] ?? '';
    const sessionWorkbenchStyles = styles.match(/\.workbench:has\(\.main-session-grid\)\s*\{([^}]*)\}/u)?.[1] ?? '';
    const collapsedWorkbenchStyles = styles.match(/\.app-shell\.sidebar-collapsed \.workbench,\s*\.app-shell\.sidebar-collapsed \.workbench:has\(\.main-session-grid\)\s*\{([^}]*)\}/u)?.[1] ?? '';
    expect(sidebarStyles).toContain('border-radius: var(--content-surface-radius) 0 0 var(--content-surface-radius)');
    expect(workbenchStyles).toContain('border-left: 1px solid var(--panel-border)');
    expect(workbenchStyles).toContain('border-radius: 0 var(--content-surface-radius) var(--content-surface-radius) 0');
    expect(sessionWorkbenchStyles).toContain('border-radius: 0 var(--content-surface-radius) var(--content-surface-radius) 0');
    expect(collapsedWorkbenchStyles).toContain('margin-left: 0');
    expect(collapsedWorkbenchStyles).toContain('border-left: 0');
    expect(collapsedWorkbenchStyles).toContain('border-radius: var(--content-surface-radius)');
  });

  it('uses smaller type in the compact session summary and a divider in its expanded view', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const compactSummary = styles.match(/\.main-session-grid\[data-session-view-state='session'\] \.session-summary-panel\s*\{([^}]*)\}/u)?.[1] ?? '';
    const expandedColumn = styles.match(/\.main-session-grid\[data-session-view-state='session'\]\.research-details-open \.research-side-column\s*\{([^}]*)\}/u)?.[1] ?? '';
    expect(compactSummary).toContain('--session-summary-font-size: 0.9rem');
    expect(compactSummary).toContain('--session-summary-tooltip-font-size: 0.7rem');
    expect(expandedColumn).toContain('border-left: 1px solid var(--panel-border)');
    for (const selector of ['session-summary-title', 'session-summary-duration', 'session-summary-item', 'session-summary-meta', 'session-memory-type-item']) {
      expect(styles).toMatch(new RegExp(`\\.${selector}\\s*\\{[^}]*font-size: var\\(--session-summary-font-size, 1rem\\);`, 'u'));
    }
  });

  it('opens Agent Settings on General instead of retaining the previous section', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const openSettings = appSource.match(/const openSettings = useCallback\(\(\) => \{([\s\S]*?)\n  \}, \[/u)?.[1] ?? '';

    expect(openSettings).toContain("setSettingsSection('general');");
    expect(openSettings.indexOf("setSettingsSection('general');"))
      .toBeLessThan(openSettings.indexOf('setSettingsOpen(true);'));
  });

  it('aligns header labels with content without crossing the menu controls', () => {
    expect(rightmostHeaderMenuControl(8, [42, 83, 128, 194])).toBe(194);
    expect(rightmostHeaderMenuControl(8, [])).toBe(8);
    expect(headerMenuInlineEnd(10.2, 201.1)).toBe(199);
    expect(headerMenuInlineEnd(220, 180)).toBe(0);

    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const workspaceLabelStyles = styles.match(/\.app-header-workspace-title\s*\{([^}]*)\}/u)?.[1] ?? '';
    const secondaryLabelStyles = styles.match(/\.app-header-session-title\s*\{([^}]*)\}/u)?.[1] ?? '';
    const sharedSessionSurfaceStyles = styles.match(/\.app-header-session-title,\s*\.app-header-surface-title\s*\{([^}]*)\}/u)?.[1] ?? '';
    expect(styles).toContain('--main-content-inline-start: calc(var(--navigation-rail-width) + var(--sidebar-width))');
    expect(styles).toMatch(/\.window-menu \.sidebar-toggle-button\s*\{[^}]*margin-left: 6px;/u);
    expect(styles).toMatch(/\.app-shell\.sidebar-collapsed\s*\{\s*--main-content-inline-start: var\(--navigation-rail-width\);/u);
    expect(styles).toContain('left: max(var(--main-content-inline-start), var(--header-menu-inline-end));');
    expect(styles).toMatch(/\.app-header-view-icon\s*\{[^}]*margin-inline: 6px;/u);
    expect(styles).toMatch(/\.app-header-divider\s*\{[^}]*height: 20px;/u);
    for (const labelStyles of [workspaceLabelStyles, secondaryLabelStyles]) {
      expect(labelStyles).toContain('height: 26px');
      expect(labelStyles).toContain('align-self: center');
      expect(labelStyles).toContain('box-sizing: border-box');
      expect(labelStyles).toContain('padding: 0 6px');
    }
    expect(workspaceLabelStyles).toContain('line-height: 1');
    expect(workspaceLabelStyles).toContain('font-size: 1rem');
    expect(sharedSessionSurfaceStyles).toContain('line-height: 1');
    expect(sharedSessionSurfaceStyles).toContain('font-size: 1rem');
    expect(sharedSessionSurfaceStyles).toContain('font-weight: 400');
    expect(styles).toMatch(/@media \(max-width: 820px\)[\s\S]*?\.app-shell\s*\{\s*--main-content-inline-start: var\(--navigation-rail-width\);/u);
    expect(styles).not.toContain('left: 180px;');
  });

  it('shows the detected default workspace editor beside the right sidenav control', () => {
    const header = renderToStaticMarkup(createElement(TopBar, {
      sidebarCollapsed: false,
      workspaceOpen: true,
      newResearchLabel: 'New Research',
      newResearchDisabled: false,
      workspaces: [],
      workspaceRegistryLoading: false,
      rightSidenavAvailable: true,
      rightSidenavExpanded: false,
      contextualTitleVisible: false,
      staticContextTitle: null,
      platform: 'win32',
      workspaceName: 'Parser',
      workspaceViewTitle: null,
      activeRunDetail: null,
      profilingEnabled: false,
      workspaceEditors: {
        editors: [
          { id: 'vscode', name: 'Visual Studio Code', iconDataUrl: 'data:image/png;base64,dnNjb2Rl' },
          { id: 'cursor', name: 'Cursor', iconDataUrl: 'data:image/png;base64,Y3Vyc29y' }
        ],
        defaultEditorId: 'cursor'
      },
      onOpenProfiling: () => undefined,
      onOpenWorkspaceInEditor: () => undefined,
      onAddWorkspace: () => undefined,
      onToggleRightSidenav: () => undefined,
      onToggleSidebar: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined,
      onOpenQuickChat: () => undefined
    }));

    expect(header).toContain('editor-launch-available');
    expect(header).toContain('Open primary workspace directory in Cursor');
    expect(header).toContain('data-tooltip="Open primary workspace directory in Cursor"');
    expect(header).toContain('data-tooltip="Choose editor"');
    expect(header).toContain('data-tooltip="Show detailed sidebar"');
    expect(header).toContain('data-tooltip="Minimize"');
    expect(header).toContain('data-tooltip="Maximize"');
    expect(header).toContain('data-tooltip="Close"');
    expect(header).toContain('class="workspace-editor-icon"');
    expect(header).toContain('data:image/png;base64,Y3Vyc29y');
    expect(header.indexOf('workspace-editor-control')).toBeLessThan(header.indexOf('right-sidenav-toggle-button'));
    expect(header).not.toContain('bottom-panel-toggle-button');
  });

  it('shows New Research and Quick Chat beside the sidebar toggle whenever the sidebar is collapsed', () => {
    const renderHeader = (sidebarCollapsed: boolean, workspaceOpen: boolean, newResearchDisabled = false): string => renderToStaticMarkup(createElement(TopBar, {
      sidebarCollapsed,
      workspaceOpen,
      newResearchLabel: 'New Research',
      newResearchDisabled,
      workspaces: [],
      workspaceRegistryLoading: false,
      rightSidenavAvailable: false,
      rightSidenavExpanded: false,
      contextualTitleVisible: false,
      staticContextTitle: null,
      platform: 'darwin',
      workspaceName: 'Example Workspace',
      activeRunDetail: null,
      profilingEnabled: false,
      workspaceEditors: null,
      onOpenProfiling: () => undefined,
      onOpenWorkspaceInEditor: () => undefined,
      onAddWorkspace: () => undefined,
      onToggleRightSidenav: () => undefined,
      onToggleSidebar: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined,
      onOpenQuickChat: () => undefined
    }));

    const collapsedHeader = renderHeader(true, true);
    expect(collapsedHeader.indexOf('class="sidebar-toggle-button header-icon-tooltip"')).toBeLessThan(collapsedHeader.indexOf('aria-label="New Research"'));
    expect(collapsedHeader.indexOf('aria-label="New Research"')).toBeLessThan(collapsedHeader.indexOf('aria-label="Quick Chat"'));
    for (const label of ['Show sidebar', 'New Research', 'Quick Chat']) {
      expect(collapsedHeader).toContain(`data-tooltip="${label}"`);
    }
    expect(collapsedHeader).not.toContain('title="Quick Chat"');
    expect(collapsedHeader).toContain('lucide-square-pen');
    expect(collapsedHeader).toContain('lucide-zap');
    expect(renderHeader(true, true, true)).toMatch(/aria-label="New Research" disabled=""/u);
    expect(renderHeader(false, true)).not.toContain('header-sidebar-action-button');
    const noWorkspaceHeader = renderHeader(true, false);
    expect(noWorkspaceHeader).toContain('aria-label="New Research" aria-haspopup="menu" aria-expanded="false"');
    expect(noWorkspaceHeader).toContain('aria-label="Quick Chat"');
    expect(noWorkspaceHeader).not.toMatch(/aria-label="New Research"[^>]*disabled/u);
    expect(renderHeader(false, false)).not.toContain('header-sidebar-action-button');
    const topBarSource = readFileSync(new URL('../src/renderer/app/TopBar.tsx', import.meta.url), 'utf8');
    expect(topBarSource).toContain('onStartNewResearchForWorkspace(workspace);');
    expect(topBarSource).toContain('newResearchPickerOpen && !workspaceOpen');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(styles).toMatch(/\.app-navigation-rail-button::after,\s*\.header-icon-tooltip::after\s*\{[^}]*content: attr\(data-tooltip\);[^}]*color: var\(--muted\);/u);
    expect(styles).toMatch(/\.header-icon-tooltip::after\s*\{[^}]*top: calc\(100% \+ 10px\);/u);
  });

  it('shows header research controls for New Research with an open workspace', () => {
    const base = {
      researchDetailsAvailable: true,
      newResearchOpen: false,
      workspaceOpen: true,
      settingsOpen: false,
      fleetOpen: false,
      reportsOpen: false,
      automationsOpen: false,
      pluginsOpen: false
    };
    expect(shouldShowHeaderResearchControls(base)).toBe(true);
    expect(shouldShowHeaderResearchControls({ ...base, researchDetailsAvailable: false, newResearchOpen: true })).toBe(true);
    expect(shouldShowHeaderResearchControls({ ...base, researchDetailsAvailable: false, newResearchOpen: true, workspaceOpen: false })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, newResearchOpen: true, settingsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, settingsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, fleetOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, reportsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, automationsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, pluginsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, researchDetailsAvailable: false })).toBe(false);

    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(styles).toMatch(/\.workspace-editor-control\s*\{[^}]*margin: 0 16px 0 0;/u);
    expect(styles).toMatch(/\.window-control-button\.right-sidenav-toggle-button\s*\{[^}]*margin: 0;/u);
    expect(styles).toMatch(/\.top-bar-darwin\.right-sidenav-available \.window-controls\s*\{[^}]*padding-right: 12px;/u);
    expect(styles).not.toMatch(/\.top-bar-custom-controls\.right-sidenav-available \.window-controls\s*\{[^}]*padding-right:/u);
    expect(styles).toMatch(/\.top-bar\s*\{[^}]*--header-control-active-background: color-mix\(in srgb, var\(--text\) 4\.5%, transparent\);/u);
    expect(styles).toMatch(/\.workspace-editor-open-button,\s*\.workspace-editor-menu-button\s*\{[^}]*background: var\(--header-control-active-background\);/u);
    expect(styles).toMatch(
      /\.sidebar-toggle-button\[aria-pressed='true'\],\s*\.window-control-button\.right-sidenav-toggle-button\[aria-pressed='true'\]\s*\{[^}]*background: var\(--header-control-active-background\);/u
    );
  });

  it('keeps the workbench at full height beneath the header', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(styles).toContain('--app-header-height: 44px');
    expect(styles).toContain('grid-template-rows: var(--app-header-height) minmax(0, 1fr) 6px;');
    expect(styles).toMatch(/\.top-bar\s*\{[^}]*min-height: var\(--app-header-height\);/u);
    expect(styles).toMatch(/\.window-controls\s*\{[^}]*align-items: center;/u);
    const mainSource = readFileSync(new URL('../src/main/index.ts', import.meta.url), 'utf8');
    expect(mainSource).toContain('trafficLightPosition: { x: 16, y: 16 }');
    expect(styles).toMatch(/\.workbench\s*\{[^}]*margin: 0 6px 0 0;[^}]*padding: 0 12px;/u);
    expect(styles).toMatch(/\.sidebar\s*\{[^}]*grid-row: 2;/u);
  });

  it('hides the right-sidenav toggle when it is unavailable', () => {
    const header = renderToStaticMarkup(createElement(TopBar, {
      sidebarCollapsed: false,
      workspaceOpen: true,
      newResearchLabel: 'New Research',
      newResearchDisabled: false,
      workspaces: [],
      workspaceRegistryLoading: false,
      rightSidenavAvailable: false,
      rightSidenavExpanded: false,
      contextualTitleVisible: false,
      staticContextTitle: null,
      platform: 'win32',
      workspaceName: 'Parser',
      workspaceViewTitle: null,
      activeRunDetail: null,
      profilingEnabled: false,
      workspaceEditors: null,
      onOpenProfiling: () => undefined,
      onOpenWorkspaceInEditor: () => undefined,
      onAddWorkspace: () => undefined,
      onToggleRightSidenav: () => undefined,
      onToggleSidebar: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined,
      onOpenQuickChat: () => undefined
    }));
    expect(header).not.toContain('right-sidenav-toggle-button');
  });

  it('hides the right-sidenav toggle when research controls are unavailable', () => {
    const header = renderToStaticMarkup(createElement(TopBar, {
      sidebarCollapsed: false,
      workspaceOpen: true,
      newResearchLabel: 'New Research',
      newResearchDisabled: false,
      workspaces: [],
      workspaceRegistryLoading: false,
      rightSidenavAvailable: false,
      rightSidenavExpanded: false,
      contextualTitleVisible: true,
      staticContextTitle: null,
      platform: 'darwin',
      workspaceName: 'Parser',
      workspaceViewTitle: null,
      activeRunDetail: null,
      profilingEnabled: false,
      workspaceEditors: null,
      onOpenProfiling: () => undefined,
      onOpenWorkspaceInEditor: () => undefined,
      onAddWorkspace: () => undefined,
      onToggleRightSidenav: () => undefined,
      onToggleSidebar: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined,
      onOpenQuickChat: () => undefined
    }));
    expect(header).not.toContain('right-sidenav-toggle-button');
  });

  it('selects active run state and detail only when ids match', () => {
    const snapshot = workspaceSnapshot('run_active', 'active');
    const detail = runDetail('run_active');

    expect(selectedRunStatus(snapshot, 'run_active')).toBe('active');
    expect(selectedRunStatus(snapshot, 'run_missing')).toBeNull();
    expect(selectedRunStatus(null, 'run_active')).toBeNull();
    expect(activeRunDetailForSelection(detail, 'run_active')).toBe(detail);
    expect(activeRunDetailForSelection(detail, 'run_other')).toBeNull();
  });

  it('uses a refreshed active session row to resume polling after a terminal track epoch', () => {
    const snapshot = workspaceSnapshot('run_track', 'completed');
    const registrySessions = [{ runId: 'run_track', workspaceId: snapshot.workspace.workspaceId,
      status: 'active' as const }];

    expect(selectedRunStatus(snapshot, 'run_track', registrySessions)).toBe('active');
    expect(selectedRunStatus(snapshot, 'run_track', [{ ...registrySessions[0], workspaceId: 'workspace_other' }]))
      .toBe('completed');
    expect(selectedRunStatus(snapshot, 'run_track', [{ ...registrySessions[0], status: 'stopped' }]))
      .toBe('completed');
  });

  it('builds shell classes from heat, chrome, and pane state', () => {
    expect(
      appShellClassName({
        sessionHeat: 'high',
        sessionInProgress: true,
        platform: 'linux',
        windowChromeState: { isMaximized: true, isFullScreen: false },
        sidebarCollapsed: true
      })
    ).toBe('app-shell session-heat-high session-in-progress platform-linux window-edge-flush sidebar-collapsed');

    expect(
      appShellClassName({
        sessionHeat: 'none',
        sessionInProgress: false,
        platform: 'darwin',
        windowChromeState: { isMaximized: false, isFullScreen: true },
        sidebarCollapsed: false
      })
    ).toBe('app-shell session-heat-none platform-darwin window-edge-flush window-full-screen');
  });

  it('resolves window control platform fallbacks', () => {
    const snapshot = workspaceSnapshot('run_test', 'completed', 'win32');
    const host = { platform: 'darwin' } as HostEnvironment;

    expect(windowControlPlatformForState(snapshot, host)).toBe('win32');
    expect(windowControlPlatformForState(null, host)).toBe('darwin');
    expect(windowControlPlatformForState(null, null)).toBe('linux');
  });
});

function workspaceSnapshot(
  runId: string,
  status: string,
  platform: HostEnvironment['platform'] = 'linux'
): WorkspaceSnapshot {
  return {
    workspace: {
      hostEnvironment: { platform }
    },
    runs: [{ run: { id: runId, status } }]
  } as unknown as WorkspaceSnapshot;
}

function runDetail(runId: string): RunDetail {
  return {
    run: { id: runId }
  } as unknown as RunDetail;
}
