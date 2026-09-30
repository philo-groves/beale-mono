import { memo } from 'react';
import type { JSX } from 'react';
import { CalendarClock, FileText, House, Plug, Settings } from 'lucide-react';

export type AppNavigationDestination = 'home' | 'automations' | 'reporting' | 'plugins' | 'settings';

const outlineIcons = {
  home: House,
  automations: CalendarClock,
  reporting: FileText,
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
      {destination === 'reporting' && (
        <path d="M6 2h8l6 6v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2zm2 9v1.5h8V11zm0 4v1.5h8V15z" fillRule="evenodd" />
      )}
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
  reportsOpen,
  automationsOpen,
  pluginsOpen
}: {
  settingsOpen: boolean;
  reportsOpen: boolean;
  automationsOpen: boolean;
  pluginsOpen: boolean;
}): AppNavigationDestination {
  if (settingsOpen) return 'settings';
  if (reportsOpen) return 'reporting';
  if (automationsOpen) return 'automations';
  if (pluginsOpen) return 'plugins';
  return 'home';
}

export const AppNavigationRail = memo(function AppNavigationRail({
  active,
  onOpenHome,
  onOpenAutomations,
  onOpenReporting,
  onOpenPlugins,
  onOpenSettings
}: {
  active: AppNavigationDestination;
  onOpenHome: () => void;
  onOpenAutomations: () => void;
  onOpenReporting: () => void;
  onOpenPlugins: () => void;
  onOpenSettings: () => void;
}): JSX.Element {
  return (
    <nav className="app-navigation-rail" aria-label="Main navigation">
      <div className="app-navigation-rail-top">
        <button type="button" className={`app-navigation-rail-button${active === 'home' ? ' active' : ''}`} title="Home" aria-label="Home" aria-current={active === 'home' ? 'page' : undefined} onClick={onOpenHome}>
          <NavigationIcon destination="home" active={active === 'home'} />
        </button>
        <div className="app-navigation-rail-sections">
          <button type="button" className={`app-navigation-rail-button${active === 'automations' ? ' active' : ''}`} title="Automations" aria-label="Automations" aria-current={active === 'automations' ? 'page' : undefined} onClick={onOpenAutomations}>
            <NavigationIcon destination="automations" active={active === 'automations'} />
          </button>
          <button type="button" className={`app-navigation-rail-button${active === 'reporting' ? ' active' : ''}`} title="Reporting" aria-label="Reporting" aria-current={active === 'reporting' ? 'page' : undefined} onClick={onOpenReporting}>
            <NavigationIcon destination="reporting" active={active === 'reporting'} />
          </button>
          <button type="button" className={`app-navigation-rail-button${active === 'plugins' ? ' active' : ''}`} title="Plugins" aria-label="Plugins" aria-current={active === 'plugins' ? 'page' : undefined} onClick={onOpenPlugins}>
            <NavigationIcon destination="plugins" active={active === 'plugins'} />
          </button>
        </div>
      </div>
      <button type="button" className={`app-navigation-rail-button${active === 'settings' ? ' active' : ''}`} title="Agent Settings" aria-label="Agent Settings" aria-current={active === 'settings' ? 'page' : undefined} onClick={onOpenSettings}>
        <NavigationIcon destination="settings" active={active === 'settings'} />
      </button>
    </nav>
  );
});
