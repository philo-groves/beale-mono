import { memo } from 'react';
import type { JSX } from 'react';
import { BookOpen, CalendarClock, House, ListChecks, Plug, Settings } from 'lucide-react';

export type AppNavigationDestination = 'home' | 'automations' | 'topics' | 'workflows' | 'plugins' | 'settings';

const outlineIcons = {
  home: House,
  automations: CalendarClock,
  topics: BookOpen,
  workflows: ListChecks,
  plugins: Plug,
  settings: Settings
};

function NavigationIcon({ destination, active }: { destination: AppNavigationDestination; active: boolean }): JSX.Element {
  if (!active) {
    const Icon = outlineIcons[destination];
    return <Icon size={18} aria-hidden="true" />;
  }

  return (
    <svg aria-hidden="true" className="app-navigation-rail-filled-icon" fill="currentColor" height="18" viewBox="0 0 24 24" width="18" xmlns="http://www.w3.org/2000/svg">
      {destination === 'home' && (
        <path d="M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2zM10 21h4v-7a2 2 0 0 0-4 0z" fillRule="evenodd" />
      )}
      {destination === 'automations' && (
        <>
          <path d="M5 4h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM16 9a7 7 0 1 0 0 14 7 7 0 0 0 0-14z" fillRule="evenodd" />
          <path d="M8 2v4m8-4v4" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="2.5" />
          <circle cx="16" cy="16" fill="none" r="6" stroke="currentColor" strokeWidth="2" />
          <path d="M16 13v3l2 1.5" fill="none" stroke="currentColor" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.8" />
        </>
      )}
      {destination === 'topics' && (
        <>
          <path d="M3 4.5c3.2-1.2 6.2-.8 9 1.2v15c-2.8-2-5.8-2.4-9-1.2z" />
          <path d="M21 4.5c-3.2-1.2-6.2-.8-9 1.2v15c2.8-2 5.8-2.4 9-1.2z" />
        </>
      )}
      {destination === 'workflows' && <path d="M8 4h13v2H8zM8 11h13v2H8zM8 18h13v2H8zM2 3h4v4H2zM2 10h4v4H2zM2 17h4v4H2z" />}
      {destination === 'plugins' && (
        <>
          <path d="M6 8h12v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4z" />
          <path d="M9 2v6m6-6v6m-3 9v5" fill="none" stroke="currentColor" strokeLinecap="round" strokeWidth="2.5" />
        </>
      )}
      {destination === 'settings' && (
        <path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.321-1.915zM15 12a3 3 0 1 0-6 0 3 3 0 0 0 6 0z" fillRule="evenodd" />
      )}
    </svg>
  );
}

export function resolveAppNavigationDestination({
  settingsOpen,
  automationsOpen,
  topicsOpen,
  workflowsOpen = false,
  pluginsOpen
}: {
  settingsOpen: boolean;
  automationsOpen: boolean;
  topicsOpen: boolean;
  workflowsOpen?: boolean;
  pluginsOpen: boolean;
}): AppNavigationDestination {
  if (settingsOpen) return 'settings';
  if (automationsOpen) return 'automations';
  if (topicsOpen) return 'topics';
  if (workflowsOpen) return 'workflows';
  if (pluginsOpen) return 'plugins';
  return 'home';
}

export const AppNavigationRail = memo(function AppNavigationRail({
  active,
  onOpenHome,
  onOpenAutomations,
  onOpenTopics,
  onOpenWorkflows,
  onOpenPlugins,
  onOpenSettings
}: {
  active: AppNavigationDestination;
  onOpenHome: () => void;
  onOpenAutomations: () => void;
  onOpenTopics: () => void;
  onOpenWorkflows: () => void;
  onOpenPlugins: () => void;
  onOpenSettings: () => void;
}): JSX.Element {
  return (
    <nav className="app-navigation-rail" aria-label="Main navigation">
      <div className="app-navigation-rail-top">
        <button type="button" className={`app-navigation-rail-button${active === 'home' ? ' active' : ''}`} data-tooltip="Home" aria-label="Home" aria-current={active === 'home' ? 'page' : undefined} onClick={onOpenHome}>
          <NavigationIcon destination="home" active={active === 'home'} />
        </button>
        <div className="app-navigation-rail-sections">
          <button type="button" className={`app-navigation-rail-button${active === 'automations' ? ' active' : ''}`} data-tooltip="Automations" aria-label="Automations" aria-current={active === 'automations' ? 'page' : undefined} onClick={onOpenAutomations}>
            <NavigationIcon destination="automations" active={active === 'automations'} />
          </button>
          <button type="button" className={`app-navigation-rail-button${active === 'topics' ? ' active' : ''}`} data-tooltip="Topics" aria-label="Topics" aria-current={active === 'topics' ? 'page' : undefined} onClick={onOpenTopics}>
            <NavigationIcon destination="topics" active={active === 'topics'} />
          </button>
          <button type="button" className={`app-navigation-rail-button${active === 'workflows' ? ' active' : ''}`} data-tooltip="Workflows" aria-label="Workflows" aria-current={active === 'workflows' ? 'page' : undefined} onClick={onOpenWorkflows}>
            <NavigationIcon destination="workflows" active={active === 'workflows'} />
          </button>
          <button type="button" className={`app-navigation-rail-button${active === 'plugins' ? ' active' : ''}`} data-tooltip="Plugins" aria-label="Plugins" aria-current={active === 'plugins' ? 'page' : undefined} onClick={onOpenPlugins}>
            <NavigationIcon destination="plugins" active={active === 'plugins'} />
          </button>
        </div>
      </div>
      <button type="button" className={`app-navigation-rail-button${active === 'settings' ? ' active' : ''}`} data-tooltip="Agent Settings" aria-label="Agent Settings" aria-current={active === 'settings' ? 'page' : undefined} onClick={onOpenSettings}>
        <NavigationIcon destination="settings" active={active === 'settings'} />
      </button>
    </nav>
  );
});
