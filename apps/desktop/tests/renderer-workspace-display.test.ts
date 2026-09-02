import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { WorkspaceRegistryEntry, WorkspaceRegistryState, ResearchSessionSummary, WorkspaceSnapshot } from '@shared/types';
import { sessionMatchesSidebarSearch, WorkspaceSidebar } from '../src/renderer/features/workspaces/WorkspaceSidebar';
import { mainSideScrollHasOverflow, mainSideScrollTargetTop } from '../src/renderer/app/MainSideScrollRegion';
import { INSET_SCROLLBAR_SELECTOR } from '../src/renderer/hooks/useInsetScrollbarActivation';
import {
  workspaceById,
  workspaceExists,
  promptSessionTitle,
  researchSessionsForWorkspace,
  shortRelativeAge
} from '../src/renderer/view-models/workspaceDisplay';
import { testResearchProfile } from './researchProfileFixture';

describe('renderer workspace display view models', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('uses the summary sidenav transition for session-list overflow', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const overflowStyles = styles.match(/\.workspace-session-overflow\s*\{([^}]*)\}/u)?.[1] ?? '';
    const expandedStyles = styles.match(/\.workspace-session-overflow\.expanded\s*\{([^}]*)\}/u)?.[1] ?? '';
    const innerStyles = styles.match(/\.workspace-session-overflow-inner\s*\{([^}]*)\}/u)?.[1] ?? '';

    expect(overflowStyles).toContain('grid-template-rows: 0fr');
    expect(overflowStyles).toContain('transition: grid-template-rows 180ms ease');
    expect(expandedStyles).toContain('grid-template-rows: 1fr');
    expect(innerStyles).toContain('overflow: hidden');
  });

  it('keeps sessions with their fixed workspace registry id', () => {
    const first = workspace('workspace_first', '/workspace/first');
    const second = workspace('workspace_second', '/workspace/second');
    const firstSession = session({ id: 'session_first', registryWorkspaceId: first.id, workspacePath: '/workspace/renamed' });
    const secondSession = session({ id: 'session_second', registryWorkspaceId: second.id, workspacePath: second.workspacePath });
    const registry: WorkspaceRegistryState = {
      registryPath: '/home/user/.beale/workspaces.json',
      workspaces: [first, second],
      researchSessions: [firstSession, secondSession]
    };

    expect(researchSessionsForWorkspace(registry, first).map((item) => item.id)).toEqual(['session_first']);
    expect(researchSessionsForWorkspace(registry, second).map((item) => item.id)).toEqual(['session_second']);
    expect(workspaceById(registry, first.id)).toBe(first);
    expect(workspaceExists(registry, second.id)).toBe(true);
    expect(workspaceExists(registry, 'missing')).toBe(false);
  });

  it('sorts sidebar sessions by minute, then alphabetically by displayed title', () => {
    const registeredWorkspace = workspace('workspace_test', '/workspace/test');
    const registry: WorkspaceRegistryState = {
      registryPath: '/home/user/.beale/workspaces.json',
      workspaces: [registeredWorkspace],
      researchSessions: [
        session({ id: 'session_zebra', title: 'Zebra Review', updatedAt: '2026-04-30T12:04:59.000Z' }),
        session({ id: 'session_newest', title: 'Newest Minute', updatedAt: '2026-04-30T12:05:00.000Z' }),
        session({ id: 'session_alpha', title: 'Alpha Review', updatedAt: '2026-04-30T12:04:01.000Z' })
      ]
    };

    expect(researchSessionsForWorkspace(registry, registeredWorkspace).map((item) => item.id)).toEqual([
      'session_newest',
      'session_alpha',
      'session_zebra'
    ]);
  });

  it('formats session titles and compact relative ages for sidebar rows', () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-30T12:00:00.000Z'));

    expect(promptSessionTitle(session({ title: 'Android Deep Link Auth Bypass', promptMarkdown: 'Audit Android links.' }))).toBe('Android Deep Link Auth Bypass');
    expect(shortRelativeAge('2026-04-30T10:00:00.000Z')).toBe('2H');
    expect(shortRelativeAge('2026-04-22T12:00:00.000Z')).toBe('1W');
  });

  it('filters sidebar sessions immediately across their searchable metadata', () => {
    const candidate = session({
      title: 'Parser Boundary Review',
      promptMarkdown: 'Audit request framing.',
      summary: 'Found an unchecked length conversion.',
      model: 'gpt-5.6-sol',
      reasoningEffort: 'xhigh'
    });

    expect(sessionMatchesSidebarSearch(candidate, 'parser length')).toBe(true);
    expect(sessionMatchesSidebarSearch(candidate, 'request framing')).toBe(true);
    expect(sessionMatchesSidebarSearch(candidate, '5.6 xhigh')).toBe(true);
    expect(sessionMatchesSidebarSearch(candidate, 'authentication')).toBe(false);
  });

  it('labels the left navigation workspace section without the profile prefix', () => {
    const profile = testResearchProfile();
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: null,
      selectedRunId: null,
      workspaceCreationActive: false,
      snapshot: {
        researchProfile: {
          profile: {
            ...profile,
            workspace: { ...profile.workspace, workspaceNoun: 'Research Workspace' }
          }
        }
      } as unknown as WorkspaceSnapshot,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));

    expect(html).toContain('<div class="workspace-list-title sidebar-list-tabs" role="tablist" aria-label="Sidebar list">');
    expect(html).toContain('role="tab" aria-selected="true" class="active">Workspaces</button>');
    expect(html).toContain('<span class="sidebar-list-tab-divider" aria-hidden="true"></span>');
    expect(html).toContain('role="tab" aria-selected="false" class="">Channels</button>');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(styles).toMatch(/\.sidebar-list-tab-divider\s*\{[^}]*width:\s*1px;[^}]*height:\s*16px;[^}]*background:\s*color-mix\(in srgb, var\(--text\) 34%, var\(--panel\)\);/s);
    expect(html).toContain('<div class="main-side-scroll sidebar-list-scroll-region">');
    expect(html).toContain('<div class="sidebar-list-scroll workspace-list-items">');
    expect(html).toContain('<div class="sidebar-list-scroll-content">');
    expect(html).toContain('class="lucide lucide-square-pen"');
    expect(html).not.toContain('class="lucide lucide-play"');
    expect(html).toContain('title="Search sessions"');
    expect(html).toContain('class="lucide lucide-search"');
    expect(html).toContain('class="workspace-list-add-button"');
    expect(html).toContain('aria-haspopup="menu"');
    expect(html).toContain('aria-expanded="false"');
    expect(html.indexOf('title="Search sessions"')).toBeLessThan(html.indexOf('title="Add research workspace"'));
    expect(html).not.toContain('Find a Session');
    expect(html).not.toContain('Workspace Information');
    expect(html).not.toContain('Research Workspaces');
  });

  it('offers create and import actions from the workspace add menu', () => {
    const sidebarSource = readFileSync(new URL('../src/renderer/features/workspaces/WorkspaceSidebar.tsx', import.meta.url), 'utf8');
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const importActionStart = appSource.indexOf('const importWorkspace = useCallback');
    const importActionEnd = appSource.indexOf('const removeActiveWorkspace', importActionStart);
    const importAction = appSource.slice(importActionStart, importActionEnd);

    expect(sidebarSource).toContain('<span>Create Workspace</span>');
    expect(sidebarSource).toContain('<span>Import Workspace</span>');
    expect(sidebarSource).toContain('<FolderPlus size={15} aria-hidden="true" />');
    expect(sidebarSource).toContain('<FolderInput size={15} aria-hidden="true" />');
    expect(sidebarSource).toContain('onAddWorkspace();');
    expect(sidebarSource).toContain('onImportWorkspace();');
    expect(importAction).toContain("window.beale.selectWorkspace('open')");
    expect(importAction).toContain('if (selection.canceled || !workspacePath) return;');
    expect(importAction).toContain('await window.beale.openWorkspace(workspacePath)');
    expect(appSource).toContain('onImportWorkspace={importWorkspace}');

    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const menuButtonStyles = styles.match(/\.section-row \.workspace-list-add-menu button\s*\{([^}]*)\}/u)?.[1] ?? '';
    expect(menuButtonStyles).toContain('font-size: 1rem');
    expect(menuButtonStyles).toContain('font-weight: 400');
  });

  it('limits sidebar scrolling to the workspace items viewport', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const sidebarStyles = styles.match(/\.sidebar\s*\{([^}]*)\}/u)?.[1] ?? '';
    const workspaceListStyles = styles.match(/\.workspace-list,\s*\.settings-sidebar-section\s*\{([^}]*)\}/u)?.[1] ?? '';
    const listScrollRegionStyles = styles.match(/\.main-side-scroll\.sidebar-list-scroll-region\s*\{([^}]*)\}/u)?.[1] ?? '';
    const listScrollStyles = styles.match(/\.sidebar-list-scroll\s*\{([^}]*)\}/u)?.[1] ?? '';

    expect(sidebarStyles).toContain('overflow: hidden');
    expect(workspaceListStyles).toContain('flex: 1 1 auto');
    expect(workspaceListStyles).toContain('grid-template-rows: auto minmax(0, 1fr)');
    expect(listScrollRegionStyles).toContain('width: calc(100% + 16px)');
    expect(listScrollRegionStyles).toContain('margin-inline: -4px -12px');
    expect(styles.indexOf('.main-side-scroll.sidebar-list-scroll-region')).toBeLessThan(styles.indexOf('.main-side-scroll {'));
    expect(listScrollStyles).toContain('width: 100%');
    expect(listScrollStyles).toContain('height: 100%');
    expect(listScrollStyles).toContain('overflow-y: scroll');
    expect(listScrollStyles).toContain('overscroll-behavior: contain');
    expect(styles).toMatch(/\.sidebar-list-scroll-content\s*\{[^}]*width: 100%/u);
    expect(styles).toMatch(/\.sidebar-list-scroll-region::before,\s*\.sidebar-list-scroll-region::after\s*\{[^}]*display: none/u);
    expect(styles).toMatch(/\.sidebar-list-scroll-region\.has-top-fade \.sidebar-list-scroll\s*\{[^}]*mask-image: linear-gradient/u);
    expect(styles).toMatch(/\.sidebar-list-scroll-region\.has-bottom-fade \.sidebar-list-scroll\s*\{[^}]*mask-image: linear-gradient/u);
    expect(styles).toMatch(/\.sidebar-list-scroll-region\.has-top-fade\.has-bottom-fade \.sidebar-list-scroll\s*\{[^}]*mask-image: linear-gradient/u);
    expect(styles).not.toMatch(/\.sidebar-list-scroll-region::(?:before|after)\s*\{[^}]*background:/u);
    expect(styles).toMatch(/\.sidebar-list-scroll \.workspace-item-row,\s*\.sidebar-list-scroll \.workspace-session-item\s*\{[^}]*width: 100%;[^}]*margin-inline: 0;/u);
    expect(styles).toMatch(/\.sidebar-list-scroll-region\.has-overflow \.sidebar-list-scroll:where\(:hover, :focus, :focus-within, \.scrollbar-active\)/u);
    expect(INSET_SCROLLBAR_SELECTOR).toContain('.sidebar-list-scroll');
    expect(INSET_SCROLLBAR_SELECTOR).not.toContain('.sidebar,');
    expect(mainSideScrollHasOverflow(300, 200)).toBe(true);
    expect(mainSideScrollHasOverflow(200, 200)).toBe(false);
    expect(mainSideScrollTargetTop(100, 0, 260)).toBe(160);
    expect(mainSideScrollTargetTop(100, 50, 80)).toBe(30);
  });

  it('keeps the inline session-search pill the same height as the workspace header', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const headerStyles = styles.match(/\.workspace-list-header\s*\{([^}]*)\}/u)?.[1] ?? '';
    const openHeaderStyles = styles.match(/\.workspace-list-header\.search-open\s*\{([^}]*)\}/u)?.[1] ?? '';
    const searchStyles = styles.match(/\.workspace-list-search\s*\{([^}]*)\}/u)?.[1] ?? '';
    const searchInputStyles = styles.match(/\.workspace-list-search > input\s*\{([^}]*)\}/u)?.[1] ?? '';
    const actionStyles = styles.match(/\.workspace-list-header-actions\s*\{([^}]*)\}/u)?.[1] ?? '';

    expect(headerStyles).toContain('height: 28px');
    expect(openHeaderStyles).toContain('gap: 0');
    expect(searchStyles).toContain('height: 28px');
    expect(searchStyles).toContain('box-sizing: border-box');
    expect(searchStyles).toContain('grid-template-columns: 13px minmax(0, 1fr) 26px');
    expect(searchStyles).toContain('border-radius: 999px');
    expect(searchStyles).toContain('margin: 0');
    expect(searchStyles).toContain('padding-left: 7px');
    expect(searchInputStyles).toContain('box-sizing: border-box');
    expect(searchInputStyles).toContain('margin: 0');
    expect(searchInputStyles).toContain('padding: 3px 4px');
    expect(actionStyles).toContain('gap: 0');
  });

  it('matches left-sidebar utility row height to workspace session rows', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const utilityStyles = styles.match(/\.sidebar-utility-button\s*\{([^}]*)\}/u)?.[1] ?? '';
    const sessionStyles = styles.match(/(?:^|\n)\.workspace-session-item\s*\{([^}]*)\}/u)?.[1] ?? '';
    const statusBarStyles = styles.match(/(?:^|\n)\.status-bar\s*\{([^}]*)\}/u)?.[1] ?? '';

    expect(utilityStyles).toContain('min-height: 30px');
    expect(utilityStyles).toContain('padding: 5px 4px');
    expect(sessionStyles).toContain('min-height: 30px');
    expect(sessionStyles).toContain('padding: 5px 4px');
    expect(statusBarStyles).toContain('height: 30px');
  });

  it('shows registry loading state instead of an empty workspace list during startup', () => {
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: null,
      workspaceRegistryLoading: true,
      selectedRunId: null,
      workspaceCreationActive: false,
      snapshot: null,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));

    expect(html).toContain('role="tab" aria-selected="true" class="active">Workspaces</button>');
    expect(html).toContain('<span class="workspace-list-title-loading" role="status" aria-label="Loading workspaces">');
    expect(html).toContain('lucide-loader-circle');
    expect(html).not.toContain('workspace-list-loading');
    expect(html).not.toContain('<span>Loading workspaces…</span>');
    expect(html).not.toContain('No Workspaces Yet');
  });

  it('marks a workspace active only while its dashboard is selected', () => {
    const profile = testResearchProfile();
    const registeredWorkspace = { ...workspace('workspace_test', '/workspace/test'), workspaceName: 'Snapchat' };
    const registry: WorkspaceRegistryState = {
      registryPath: '/home/user/.beale/workspaces.json',
      workspaces: [registeredWorkspace],
      researchSessions: Array.from({ length: 5 }, (_, index) => session({
        id: `session_${index}`,
        runId: `run_${index}`,
        registryWorkspaceId: registeredWorkspace.id
      }))
    };
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: registry,
      selectedRunId: null,
      workspaceCreationActive: false,
      snapshot: {
        workspace: { workspacePath: registeredWorkspace.workspacePath },
        researchProfile: { profile },
        runs: []
      } as unknown as WorkspaceSnapshot,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));

    expect(html).toMatch(/class="workspace-item-row active\b/u);
    expect(html).toContain('class="workspace-session-overflow" aria-hidden="true" inert=""');
    expect(html).toContain('class="session-memory-type-toggle" aria-expanded="false">Show 1 more</button>');
    expect(html).not.toContain('More Sessions...');
    expect(html).not.toContain('More Snapchat Sessions');
    expect(html).not.toContain('More Research Sessions');
    expect(html).toContain('class="workspace-new-research-button"');
    expect(html).toContain('title="Start new research in Snapchat"');
    expect(html).toContain('aria-label="Start new research in Snapchat"');
    expect(html.match(/lucide-square-pen/gu)).toHaveLength(2);
    expect(html).not.toContain('workspace-menu-button');
    expect(html.match(/aria-haspopup="menu"/gu)).toHaveLength(1);
  });

  it('marks only the workspace creation icon active while New Workspace is rendered', () => {
    const profile = testResearchProfile();
    const registeredWorkspace = { ...workspace('workspace_test', '/workspace/test'), workspaceName: 'Snapchat' };
    const registry: WorkspaceRegistryState = {
      registryPath: '/home/user/.beale/workspaces.json',
      workspaces: [registeredWorkspace],
      researchSessions: [session({
        id: 'session_selected',
        runId: 'run_selected',
        registryWorkspaceId: registeredWorkspace.id
      })]
    };
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: registry,
      selectedRunId: 'run_selected',
      workspaceCreationActive: true,
      automationsActive: true,
      snapshot: {
        workspace: { workspacePath: registeredWorkspace.workspacePath },
        researchProfile: { profile },
        runs: []
      } as unknown as WorkspaceSnapshot,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));
    const activeClasses = html.match(/class="[^"]*\bactive\b[^"]*"/gu) ?? [];
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');

    expect(activeClasses).toEqual(['class="active"', 'class="workspace-list-add-button active"']);
    expect(html).toContain('aria-current="page"');
    expect(html.match(/aria-current="page"/gu)).toHaveLength(1);
    expect(appSource).toContain('workspaceCreationActive={workspaceDraft !== null}');
    expect(styles).toMatch(/\.workspace-list-add-button\.active,[\s\S]*?background: var\(--panel\);/u);
  });

  it('shows New Research as the active placeholder session for the loaded workspace', () => {
    const profile = testResearchProfile();
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const registeredWorkspace = { ...workspace('workspace_test', '/workspace/test'), workspaceName: 'Snapchat' };
    const registry: WorkspaceRegistryState = {
      registryPath: '/home/user/.beale/workspaces.json',
      workspaces: [registeredWorkspace],
      researchSessions: [session({
        id: 'session_old',
        runId: 'run_old',
        title: 'Old Session',
        registryWorkspaceId: registeredWorkspace.id
      })]
    };
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: registry,
      selectedRunId: 'run_old',
      workspaceCreationActive: false,
      newResearchActive: true,
      snapshot: {
        workspace: { workspacePath: registeredWorkspace.workspacePath },
        researchProfile: {
          profile: {
            ...profile,
            presentation: { ...profile.presentation, newResearchLabel: 'New Research' }
          }
        },
        runs: []
      } as unknown as WorkspaceSnapshot,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));

    expect(html).toContain('class="workspace-session-item workspace-new-research-session-item active" aria-current="page"');
    expect(html).toContain('class="workspace-new-research-session-indent"');
    expect(html).toContain('<span class="workspace-session-title">New Research</span>');
    expect(html).toContain('Old Session');
    expect(html).not.toContain('class="workspace-session-item active"');
    expect(styles).toMatch(/\.workspace-new-research-session-item \.workspace-session-title\s*\{[^}]*font-style: italic;/u);
  });

  it('switches workspaces before opening New Research from a workspace row', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const startActionSource = appSource.match(
      /const startNewResearch = useCallback[\s\S]*?const startNewResearchForWorkspace/u
    )?.[0] ?? '';
    const actionSource = appSource.match(
      /const startNewResearchForWorkspace = useCallback[\s\S]*?const startNewResearchFromSuggestion/u
    )?.[0] ?? '';

    expect(startActionSource).toContain('closeWorkspaceOnboarding();');
    expect(actionSource).toContain('snapshot?.workspace.workspacePath === workspace.workspacePath');
    expect(actionSource).toContain('applySnapshot(await window.beale.openRegisteredWorkspace(workspace.id));');
    expect(actionSource.indexOf('applySnapshot(await window.beale.openRegisteredWorkspace(workspace.id));'))
      .toBeLessThan(actionSource.lastIndexOf('startNewResearch();'));
  });

  it('closes New Workspace before navigating from the left sidenav', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const workspaceActionsSource = readFileSync(
      new URL('../src/renderer/hooks/useWorkspaceActions.ts', import.meta.url),
      'utf8'
    );
    const callbacks = [
      ['openPlugins', 'runAgentPluginAction'],
      ['openReports', 'reportingWorkspaceCatalogKey'],
      ['openSettings', 'openProfiling'],
      ['openAutomations', 'selectAutomation']
    ] as const;

    for (const [callbackName, nextDeclaration] of callbacks) {
      const start = appSource.indexOf(`const ${callbackName} = useCallback`);
      const end = appSource.indexOf(`const ${nextDeclaration}`, start);
      expect(start).toBeGreaterThan(-1);
      expect(appSource.slice(start, end)).toContain('closeWorkspaceOnboarding();');
    }

    const workspaceHandlerStart = appSource.indexOf('const openWorkspaceFromSidebar = useCallback');
    const sessionHandlerStart = appSource.indexOf('const openResearchSessionFromSidebar = useCallback', workspaceHandlerStart);
    const sidebarHandlersEnd = appSource.indexOf('const importWorkspace = useCallback', sessionHandlerStart);
    expect(workspaceHandlerStart).toBeGreaterThan(-1);
    expect(sessionHandlerStart).toBeGreaterThan(workspaceHandlerStart);
    expect(appSource.slice(workspaceHandlerStart, sidebarHandlersEnd).match(/closeWorkspaceOnboarding\(\);/gu)).toHaveLength(2);

    const sessionStart = workspaceActionsSource.indexOf('const openResearchSession = useCallback');
    const sessionEnd = workspaceActionsSource.indexOf('const removeRegisteredWorkspace = useCallback', sessionStart);
    const sessionAction = workspaceActionsSource.slice(sessionStart, sessionEnd);
    expect(sessionAction.indexOf('setWorkspaceDraft(null);'))
      .toBeLessThan(sessionAction.indexOf('if (!researchSessionNeedsLoading'));
  });

  it('moves an active session spinner to the leading slot and keeps its timestamp on the right', () => {
    const profile = testResearchProfile();
    const registeredWorkspace = workspace('workspace_test', '/workspace/test');
    const activeSession = session({ status: 'active', registryWorkspaceId: registeredWorkspace.id });
    const registry: WorkspaceRegistryState = {
      registryPath: '/home/user/.beale/workspaces.json',
      workspaces: [registeredWorkspace],
      researchSessions: [activeSession]
    };
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: registry,
      selectedRunId: activeSession.runId,
      workspaceCreationActive: false,
      snapshot: {
        workspace: { workspacePath: registeredWorkspace.workspacePath },
        researchProfile: { profile },
        runs: []
      } as unknown as WorkspaceSnapshot,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));

    expect(html).toContain('class="workspace-session-leading-status" title="Active"');
    expect(html).toContain('class="lucide lucide-refresh-cw"');
    expect(html).toContain('class="workspace-session-age"');
    expect(html.indexOf('workspace-session-leading-status')).toBeLessThan(html.indexOf('workspace-session-title'));
    expect(html.indexOf('workspace-session-age')).toBeGreaterThan(html.indexOf('workspace-session-title'));
  });

  it('uses the leading slot for an unviewed result dot and leaves viewed results blank', () => {
    const registeredWorkspace = workspace('workspace_test', '/workspace/test');
    const registry: WorkspaceRegistryState = {
      registryPath: '/home/user/.beale/workspaces.json',
      workspaces: [registeredWorkspace],
      researchSessions: [
        session({ id: 'session_unviewed', runId: 'run_unviewed', resultViewedAt: null }),
        session({ id: 'session_viewed', runId: 'run_viewed', resultViewedAt: '2026-04-30T02:00:00.000Z' })
      ]
    };
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: registry,
      selectedRunId: null,
      workspaceCreationActive: false,
      snapshot: null,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));

    expect(html.match(/workspace-session-unviewed-dot/gu)).toHaveLength(1);
    expect(html).toContain('aria-label="Session result not viewed"');
  });
});

function workspace(id: string, workspacePath: string): WorkspaceRegistryEntry {
  return {
    id,
    workspacePath,
    workspaceId: id.replace('workspace_', 'workspace_'),
    workspaceName: id,
    scopeOwner: '',
    researchProfileId: 'security-research',
    researchKitId: 'general',
    descriptionMarkdown: '',
    rulesMarkdown: '',
    expiresAt: null,
    createdAt: '2026-04-30T00:00:00.000Z',
    updatedAt: '2026-04-30T00:00:00.000Z',
    lastOpenedAt: null,
    runCount: 0,
    lastRunAt: null
  };
}

function session(input: Partial<ResearchSessionSummary>): ResearchSessionSummary {
  return {
    id: 'session_test',
    registryWorkspaceId: 'workspace_test',
    workspacePath: '/workspace/test',
    workspaceId: 'workspace_test',
    runId: 'run_test',
    title: '',
    status: 'completed',
    runEngine: 'app-server',
    mode: 'dynamic',
    promptMarkdown: '',
    summary: '',
    model: 'gpt-5.5',
    reasoningEffort: 'xhigh',
    sandboxProfile: 'host',
    createdAt: '2026-04-30T00:00:00.000Z',
    startedAt: '2026-04-30T00:00:00.000Z',
    endedAt: '2026-04-30T01:00:00.000Z',
    updatedAt: '2026-04-30T01:00:00.000Z',
    resultViewedAt: null,
    ...input,
    finalDisposition: input.finalDisposition ?? null
  };
}
