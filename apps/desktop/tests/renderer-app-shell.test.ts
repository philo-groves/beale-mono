import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { HostEnvironment, RunDetail, RunRow, WorkspaceSnapshot } from '@shared/types';
import { AppHeaderTitle, StaticAppHeaderTitle, type AppHeaderViewIcon } from '../src/renderer/app/AppHeaderTitle';
import { BottomPanel, DEFAULT_BOTTOM_PANEL_OPEN } from '../src/renderer/app/BottomPanel';
import { AppNavigationRail, resolveAppNavigationDestination } from '../src/renderer/app/AppNavigationRail';
import { headerMenuInlineEnd, rightmostHeaderMenuControl, TopBar } from '../src/renderer/app/TopBar';
import { SessionOverviewDialog } from '../src/renderer/features/sessions/SessionOverviewDialog';
import {
  activeRunDetailForSelection,
  appShellClassName,
  selectedRunStatus,
  shouldShowHeaderResearchControls,
  workspaceHasLiveResearchRun,
  windowControlPlatformForState
} from '../src/renderer/view-models/appShell';

describe('renderer app shell view model', () => {
  it('matches header icons to workspace and sidenav destinations', () => {
    const workspaceHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'ExampleCo iOS Parser',
      workspaceViewTitle: 'Memory',
      detail: null,
      topicTitle: null
    }));
    expect(workspaceHeader).toContain('lucide-folder');
    expect(workspaceHeader).toContain('aria-label="ExampleCo iOS Parser, Memory"');
    expect(workspaceHeader).toContain('title="ExampleCo iOS Parser"');
    expect(workspaceHeader).toContain('title="Memory"><span>Memory</span>');

    const newResearchHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'Parser',
      workspaceViewTitle: 'New Research',
      detail: { run: { id: 'run_old', title: 'Old Session', promptMarkdown: 'Old prompt' } } as RunDetail,
      topicTitle: 'Old Topic'
    }));
    expect(newResearchHeader).toContain('aria-label="Parser, New Research"');
    expect(newResearchHeader).not.toContain('Old Session');
    expect(newResearchHeader).not.toContain('Old Topic');

    const topicHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'Parser',
      workspaceViewTitle: null,
      detail: null,
      topicTitle: 'parser review'
    }));
    expect(topicHeader).toContain('aria-label="Parser, parser review"');
    expect(topicHeader).toContain('class="app-header-topic-title app-header-static-title"');

    const sessionHeader = renderToStaticMarkup(createElement(AppHeaderTitle, {
      workspaceName: 'Parser',
      workspaceViewTitle: null,
      detail: { run: { id: 'run_current', title: 'Inspect parser states', promptMarkdown: 'Inspect parser states' } } as RunDetail,
      topicTitle: null,
      onOpenSessionOverview: () => undefined
    }));
    expect(sessionHeader).toContain('class="app-header-session-title app-header-session-overview-button"');
    expect(sessionHeader).toContain('aria-label="Open Session Overview for Inspect parser states"');
    const mainTitle = 'class="app-header-workspace-title app-header-static-title"';
    for (const header of [workspaceHeader, newResearchHeader, topicHeader, sessionHeader]) {
      expect(header.indexOf('class="app-header-divider"')).toBeLessThan(header.indexOf(mainTitle));
      expect(header.match(/class="app-header-divider"/gu)).toHaveLength(1);
    }

    const viewIcons: Array<[AppHeaderViewIcon, string]> = [
      ['settings', 'lucide-settings'],
      ['settings-appearance', 'lucide-palette'],
      ['settings-profiles', 'lucide-user-round-cog'],
      ['settings-providers', 'lucide-server-cog'],
      ['settings-remote', 'lucide-wifi'],
      ['settings-ticketing', 'lucide-ticket'],
      ['automations', 'lucide-calendar-clock'],
      ['topics', 'lucide-book-open'],
      ['reporting', 'lucide-file-text'],
      ['plugins', 'lucide-plug']
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

  it('renders the active session Trail projection in a wide squircle overview dialog', () => {
    const run = {
      run: {
        id: 'run_current',
        title: 'Inspect parser states',
        promptMarkdown: 'Inspect parser states',
        createdAt: '2026-08-26T12:00:00.000Z'
      },
      engine: 'pi',
      sessionRuns: [{
        id: 'session_run_current',
        runId: 'run_current',
        attemptId: 'attempt_current',
        status: 'completed',
        terminationCause: null,
        activityIntervals: [{
          id: 'interval_current',
          runId: 'run_current',
          attemptId: 'attempt_current',
          startedAt: '2026-08-26T12:00:00.000Z',
          endedAt: '2026-08-26T12:30:00.000Z'
        }]
      }]
    } as unknown as RunRow;
    const html = renderToStaticMarkup(createElement(SessionOverviewDialog, {
      memory: null,
      memoryTypes: [],
      nowMs: Date.parse('2026-08-26T13:00:00.000Z'),
      onClose: () => undefined,
      run,
      sessionHeatPreferences: { heatOverrides: {}, paletteOverrides: {} }
    }));

    expect(html).toContain('class="modal-panel wide-modal session-overview-dialog"');
    expect(html).toContain('>Session Overview</h2>');
    expect(html).toContain('<h3>Inspect parser states</h3>');
    expect(html).toContain('<p>30m</p>');
    expect(html).toContain('class="campaign-session-projection"');
    expect(html).toContain('complete session activity: 30m');
    expect(html).toContain('class="workspace-timeline-segment"');

    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(styles).toMatch(/\.modal-panel\.session-overview-dialog\s*\{[^}]*width: min\(1180px, calc\(100vw - 36px\)\);[^}]*border-radius: 34px;[^}]*corner-shape: squircle;/u);
    expect(styles).toMatch(/\.session-overview-dialog \.campaign-session-projection\s*\{[^}]*height: 72px;/u);
  });

  it('renders the compact navigation rail with Settings after the top destinations', () => {
    const onNavigate = () => undefined;
    const html = renderToStaticMarkup(createElement(AppNavigationRail, {
      active: 'settings', onOpenHome: onNavigate, onOpenAutomations: onNavigate, onOpenTopics: onNavigate,
      onOpenPlugins: onNavigate, onOpenSettings: onNavigate
    }));

    expect(html).toContain('aria-label="Main navigation"');
    for (const label of ['Home', 'Automations', 'Topics', 'Plugins', 'Agent Settings']) {
      expect(html).toContain(`aria-label="${label}"`);
      expect(html).toContain(`data-tooltip="${label}"`);
    }
    expect(html.indexOf('aria-label="Home"')).toBeLessThan(html.indexOf('aria-label="Automations"'));
    expect(html.indexOf('aria-label="Automations"')).toBeLessThan(html.indexOf('aria-label="Topics"'));
    expect(html.indexOf('aria-label="Topics"')).toBeLessThan(html.indexOf('aria-label="Plugins"'));
    expect(html.indexOf('aria-label="Plugins"')).toBeLessThan(html.indexOf('aria-label="Agent Settings"'));
    expect(html).toContain('class="app-navigation-rail-filled-icon"');
    expect(html).toContain('aria-current="page"');
    expect(html).toContain('width="18"');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const railStyles = styles.match(/\.app-navigation-rail\s*\{([^}]*)\}/u)?.[1] ?? '';
    const topStyles = styles.match(/\.app-navigation-rail-top,\s*\.app-navigation-rail-sections\s*\{([^}]*)\}/u)?.[1] ?? '';
    expect(styles).toContain('--navigation-rail-width: 50px');
    expect(railStyles).toContain('padding: 4px');
    expect(railStyles).toContain('background: transparent');
    expect(styles).not.toMatch(/\.app-shell\.sidebar-collapsed \.app-navigation-rail\s*\{[^}]*width:/u);
    expect(topStyles).toContain('gap: 6px');
    expect(styles).toContain('.app-navigation-rail-top {\n  margin-top: 8px;');
    expect(styles).toContain('.app-navigation-rail-sections {\n  margin-top: 0;');
    expect(styles).toMatch(/\.app-navigation-rail-button::after\s*\{[^}]*left: calc\(100% \+ 10px\);[^}]*content: attr\(data-tooltip\);/s);
    expect(styles).toContain('.app-navigation-rail-button:focus-visible::after');
  });

  it('fills only the active navigation icon', () => {
    const onNavigate = () => undefined;
    for (const active of ['home', 'automations', 'topics', 'plugins', 'settings'] as const) {
      const html = renderToStaticMarkup(createElement(AppNavigationRail, {
        active, onOpenHome: onNavigate, onOpenAutomations: onNavigate, onOpenTopics: onNavigate,
        onOpenPlugins: onNavigate, onOpenSettings: onNavigate
      }));
      expect(html.match(/class="app-navigation-rail-filled-icon"/gu)).toHaveLength(1);
      expect(html.match(/class="lucide lucide-/gu)).toHaveLength(4);
      expect(html.match(/aria-current="page"/gu)).toHaveLength(1);
    }
  });

  it('keeps a navigation destination active as sessions and topics open', () => {
    const base = { settingsOpen: false, automationsOpen: false, topicsOpen: false, pluginsOpen: false };
    expect(resolveAppNavigationDestination(base)).toBe('home');
    expect(resolveAppNavigationDestination({ ...base, automationsOpen: true })).toBe('automations');
    expect(resolveAppNavigationDestination({ ...base, topicsOpen: true })).toBe('topics');
    expect(resolveAppNavigationDestination({ ...base, pluginsOpen: true })).toBe('plugins');
    expect(resolveAppNavigationDestination({ ...base, settingsOpen: true })).toBe('settings');

    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    expect(appSource).toContain('resolveAppNavigationDestination({ settingsOpen, automationsOpen, topicsOpen, pluginsOpen })');
    const onNavigate = () => undefined;
    const html = renderToStaticMarkup(createElement(AppNavigationRail, {
      active: resolveAppNavigationDestination(base),
      onOpenHome: onNavigate, onOpenAutomations: onNavigate, onOpenTopics: onNavigate,
      onOpenPlugins: onNavigate, onOpenSettings: onNavigate
    }));
    expect(html).toContain('class="app-navigation-rail-button active" data-tooltip="Home"');
    expect(html.match(/aria-current="page"/gu)).toHaveLength(1);
  });

  it('rounds the outer edges of the secondary sidebar and main content', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const sidebarStyles = styles.match(/(?:^|\n)\.sidebar\s*\{([^}]*)\}/u)?.[1] ?? '';
    const workbenchStyles = styles.match(/(?:^|\n)\.workbench\s*\{([^}]*)\}/u)?.[1] ?? '';
    const sessionWorkbenchStyles = styles.match(/\.app-shell\[data-background\] \.workbench:has\(\.main-session-grid\)\s*\{([^}]*)\}/u)?.[1] ?? '';
    const collapsedWorkbenchStyles = styles.match(/\.app-shell\.sidebar-collapsed \.workbench,\s*\.app-shell\.sidebar-collapsed\[data-background\] \.workbench:has\(\.main-session-grid\)\s*\{([^}]*)\}/u)?.[1] ?? '';
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
    for (const selector of ['session-summary-title', 'session-summary-duration', 'session-summary-item', 'session-summary-meta', 'session-memory-type-item', 'session-memory-type-toggle']) {
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
    const secondaryLabelStyles = styles.match(/\.app-header-session-title,\s*\.app-header-topic-title\s*\{([^}]*)\}/u)?.[1] ?? '';
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
      expect(labelStyles).toContain('line-height: 1');
      expect(labelStyles).toContain('font-size: 1rem');
    }
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
      activeTopicTitle: null,
      profilingEnabled: false,
      bottomPanelOpen: true,
      workspaceEditors: {
        editors: [
          { id: 'vscode', name: 'Visual Studio Code', iconDataUrl: 'data:image/png;base64,dnNjb2Rl' },
          { id: 'cursor', name: 'Cursor', iconDataUrl: 'data:image/png;base64,Y3Vyc29y' }
        ],
        defaultEditorId: 'cursor'
      },
      onOpenProfiling: () => undefined,
      onToggleBottomPanel: () => undefined,
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
    expect(header).toContain('class="workspace-editor-icon"');
    expect(header).toContain('data:image/png;base64,Y3Vyc29y');
    expect(header.indexOf('workspace-editor-control')).toBeLessThan(header.indexOf('right-sidenav-toggle-button'));
    expect(header).toContain('aria-label="Hide bottom panel"');
    expect(header.indexOf('bottom-panel-toggle-button')).toBeLessThan(header.indexOf('right-sidenav-toggle-button'));
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
      activeTopicTitle: null,
      profilingEnabled: false,
      bottomPanelOpen: false,
      workspaceEditors: null,
      onOpenProfiling: () => undefined,
      onToggleBottomPanel: () => undefined,
      onOpenWorkspaceInEditor: () => undefined,
      onAddWorkspace: () => undefined,
      onToggleRightSidenav: () => undefined,
      onToggleSidebar: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined,
      onOpenQuickChat: () => undefined
    }));

    const collapsedHeader = renderHeader(true, true);
    expect(collapsedHeader.indexOf('class="sidebar-toggle-button"')).toBeLessThan(collapsedHeader.indexOf('aria-label="New Research"'));
    expect(collapsedHeader.indexOf('aria-label="New Research"')).toBeLessThan(collapsedHeader.indexOf('aria-label="Quick Chat"'));
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
  });

  it('shows header research controls for New Research with an open workspace', () => {
    const base = {
      researchDetailsAvailable: true,
      newResearchOpen: false,
      workspaceOpen: true,
      settingsOpen: false,
      reportsOpen: false,
      automationsOpen: false,
      topicsOpen: false,
      pluginsOpen: false
    };
    expect(shouldShowHeaderResearchControls(base)).toBe(true);
    expect(shouldShowHeaderResearchControls({ ...base, researchDetailsAvailable: false, newResearchOpen: true })).toBe(true);
    expect(shouldShowHeaderResearchControls({ ...base, researchDetailsAvailable: false, newResearchOpen: true, workspaceOpen: false })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, newResearchOpen: true, settingsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, settingsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, reportsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, automationsOpen: true })).toBe(false);
    expect(shouldShowHeaderResearchControls({ ...base, newResearchOpen: true, topicsOpen: true })).toBe(false);
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
      /\.sidebar-toggle-button\[aria-pressed='true'\],\s*\.window-control-button\.bottom-panel-toggle-button\[aria-pressed='true'\],\s*\.window-control-button\.right-sidenav-toggle-button\[aria-pressed='true'\]\s*\{[^}]*background: var\(--header-control-active-background\);/u
    );
  });

  it('renders a separate fixed-height terminal panel below the workbench', () => {
    expect(DEFAULT_BOTTOM_PANEL_OPEN).toBe(false);
    const panel = renderToStaticMarkup(createElement(BottomPanel, {
      open: true,
      workspacePath: 'C:\\research\\parser',
      onClose: () => undefined
    }));
    expect(panel).toContain('aria-label="Bottom panel"');
    expect(panel).toContain('lucide-square-terminal');
    expect(panel).toContain('role="tab"');
    expect(panel).toContain('aria-selected="true"');
    expect(panel).toContain('<span>Terminal</span>');
    expect(panel).toContain('class="research-side-view-tab-close"');
    expect(panel).toContain('aria-label="Close Terminal"');
    expect(panel).toContain('lucide-x');
    expect(panel).toContain('class="bottom-panel-terminal"');

    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(styles).toContain('--app-header-height: 44px');
    expect(styles).toContain('grid-template-rows: var(--app-header-height) minmax(0, 1fr) 8px 250px 6px;');
    expect(styles).toContain('grid-template-rows: var(--app-header-height) minmax(0, 1fr) 0 0 6px;');
    expect(styles).toMatch(/\.top-bar\s*\{[^}]*min-height: var\(--app-header-height\);/u);
    expect(styles).toMatch(/\.window-controls\s*\{[^}]*align-items: center;/u);
    const mainSource = readFileSync(new URL('../src/main/index.ts', import.meta.url), 'utf8');
    expect(mainSource).toContain('trafficLightPosition: { x: 16, y: 16 }');
    expect(styles).toMatch(/\.workbench\s*\{[^}]*margin: 0 6px 0 0;[^}]*padding: 0 12px;/u);
    expect(styles).toMatch(/\.bottom-panel\s*\{[^}]*grid-column: 3;[^}]*grid-row: 4;[^}]*height: 0;/u);
    expect(styles).toMatch(/\.bottom-panel\s*\{[^}]*margin: 0 6px 0 0;[^}]*padding: 0;/u);
    expect(styles).toMatch(/\.app-shell\.bottom-panel-open \.bottom-panel\s*\{[^}]*height: 250px;/u);
    expect(styles).toMatch(/\.bottom-panel-terminal\s*\{[^}]*padding-inline: 10px;/u);
    expect(styles).toMatch(/\.sidebar\s*\{[^}]*grid-row: 2 \/ 5;/u);
  });

  it('hides the bottom-panel toggle wherever the right-sidenav toggle is unavailable', () => {
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
      activeTopicTitle: null,
      profilingEnabled: false,
      bottomPanelOpen: false,
      workspaceEditors: null,
      onOpenProfiling: () => undefined,
      onToggleBottomPanel: () => undefined,
      onOpenWorkspaceInEditor: () => undefined,
      onAddWorkspace: () => undefined,
      onToggleRightSidenav: () => undefined,
      onToggleSidebar: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined,
      onOpenQuickChat: () => undefined
    }));
    expect(header).not.toContain('bottom-panel-toggle-button');
    expect(header).not.toContain('right-sidenav-toggle-button');
  });

  it('hides panel toggles when a topic is open', () => {
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
      activeTopicTitle: 'parser-review',
      profilingEnabled: false,
      bottomPanelAvailable: false,
      bottomPanelOpen: false,
      workspaceEditors: null,
      onOpenProfiling: () => undefined,
      onToggleBottomPanel: () => undefined,
      onOpenWorkspaceInEditor: () => undefined,
      onAddWorkspace: () => undefined,
      onToggleRightSidenav: () => undefined,
      onToggleSidebar: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined,
      onOpenQuickChat: () => undefined
    }));
    expect(header).not.toContain('bottom-panel-toggle-button');
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

  it('builds shell classes from heat, chrome, and pane state without animation state', () => {
    expect(
      appShellClassName({
        sessionHeat: 'high',
        sessionActive: true,
        platform: 'linux',
        windowChromeState: { isMaximized: true, isFullScreen: false },
        sidebarCollapsed: true
      })
    ).toBe('app-shell session-heat-high platform-linux session-active window-edge-flush sidebar-collapsed');

    expect(
      appShellClassName({
        sessionHeat: 'none',
        sessionActive: false,
        platform: 'darwin',
        windowChromeState: { isMaximized: false, isFullScreen: true },
        sidebarCollapsed: false
      })
    ).toBe('app-shell session-heat-none platform-darwin window-edge-flush window-full-screen');
  });

  it('keeps the window pulse active whenever the workspace has queued or active research', () => {
    expect(workspaceHasLiveResearchRun(workspaceSnapshot('run_active', 'active'))).toBe(true);
    expect(workspaceHasLiveResearchRun(workspaceSnapshot('run_queued', 'queued'))).toBe(true);
    expect(workspaceHasLiveResearchRun(workspaceSnapshot('run_paused', 'paused'))).toBe(false);
    expect(workspaceHasLiveResearchRun(workspaceSnapshot('run_completed', 'completed'))).toBe(false);
    expect(workspaceHasLiveResearchRun(null)).toBe(false);
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
