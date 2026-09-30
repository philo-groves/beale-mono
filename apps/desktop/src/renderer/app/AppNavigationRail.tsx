import { memo } from 'react';
import type { JSX } from 'react';
import { CalendarClock, FileText, House, Plug, Settings } from 'lucide-react';

export type AppNavigationDestination = 'home' | 'automations' | 'reporting' | 'plugins' | 'settings';

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
          <House size={18} aria-hidden="true" />
        </button>
        <div className="app-navigation-rail-sections">
          <button type="button" className={`app-navigation-rail-button${active === 'automations' ? ' active' : ''}`} title="Automations" aria-label="Automations" aria-current={active === 'automations' ? 'page' : undefined} onClick={onOpenAutomations}>
            <CalendarClock size={18} aria-hidden="true" />
          </button>
          <button type="button" className={`app-navigation-rail-button${active === 'reporting' ? ' active' : ''}`} title="Reporting" aria-label="Reporting" aria-current={active === 'reporting' ? 'page' : undefined} onClick={onOpenReporting}>
            <FileText size={18} aria-hidden="true" />
          </button>
          <button type="button" className={`app-navigation-rail-button${active === 'plugins' ? ' active' : ''}`} title="Plugins" aria-label="Plugins" aria-current={active === 'plugins' ? 'page' : undefined} onClick={onOpenPlugins}>
            <Plug size={18} aria-hidden="true" />
          </button>
        </div>
      </div>
      <button type="button" className={`app-navigation-rail-button${active === 'settings' ? ' active' : ''}`} title="Agent Settings" aria-label="Agent Settings" aria-current={active === 'settings' ? 'page' : undefined} onClick={onOpenSettings}>
        <Settings size={18} aria-hidden="true" />
      </button>
    </nav>
  );
});
