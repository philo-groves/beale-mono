import { useState } from 'react';
import type { JSX, PointerEvent as ReactPointerEvent } from 'react';
import { Globe2, Monitor, Plus, ServerCog } from 'lucide-react';
import type { FleetAppServerStatus, FleetState } from '@beale/app-server-runtime/protocol';
import { CollectionSidebar } from '../../app/CollectionSidebar';

export type FleetSection = 'app-servers' | 'browsers' | 'machines';
export interface FleetSelection {
  section: FleetSection;
  itemId: string | null;
}

const sections = [
  { id: 'app-servers', label: 'App Servers', icon: ServerCog },
  { id: 'browsers', label: 'Browsers', icon: Globe2 },
  { id: 'machines', label: 'Virtual Machines', icon: Monitor }
] as const;

export function FleetSidebar({ collapsed, state, statuses, selection, error, onSelect, onSetBrowserEnabled, onResizePointerDown }: {
  collapsed: boolean;
  state: FleetState | null;
  statuses: Record<string, FleetAppServerStatus>;
  selection: FleetSelection;
  error: string | null;
  onSelect: (selection: FleetSelection) => void;
  onSetBrowserEnabled: (browserId: string, enabled: boolean) => Promise<void>;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
}): JSX.Element {
  const [updatingBrowserId, setUpdatingBrowserId] = useState<string | null>(null);
  const [browserError, setBrowserError] = useState<string | null>(null);
  const updateBrowser = async (browserId: string, enabled: boolean): Promise<void> => {
    if (updatingBrowserId) return;
    setUpdatingBrowserId(browserId);
    setBrowserError(null);
    try {
      await onSetBrowserEnabled(browserId, enabled);
    } catch (caught) {
      setBrowserError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setUpdatingBrowserId(null);
    }
  };
  const select = (section: FleetSection, itemId: string | null = null): void => onSelect({ section, itemId });
  const statusFor = (id: string): FleetAppServerStatus => statuses[id] ?? { id, state: 'offline', message: 'Status unavailable.' };
  return (
    <CollectionSidebar
      title="Fleet"
      label="Fleet"
      collapsed={collapsed}
      error={error ?? browserError}
      updateKey={`${state?.appServers.length ?? 0}:${state?.browsers.length ?? 0}:${state?.machines.length ?? 0}:${selection.section}:${selection.itemId ?? ''}`}
      onResizePointerDown={onResizePointerDown}
    >
      <nav className="sidebar-list-scroll-content fleet-sidebar-sections" aria-label="Fleet sections">
        {sections.map(({ id, label, icon: Icon }) => (
          <div className="workspace-group" key={id}>
            <div className={`workspace-item-row${id === 'app-servers' ? '' : ' no-menu'}${selection.section === id && (selection.itemId === null || id === 'browsers' && selection.itemId === 'settings') ? ' active' : ''}`}>
              <button type="button" className="workspace-item" onClick={() => select(id, id === 'browsers' ? 'settings' : null)} aria-label={label}>
                <Icon size={15} aria-hidden="true" /><span>{label}</span>
              </button>
              {id === 'app-servers' ? <button type="button" className="workspace-new-research-button fleet-sidebar-add-button"
                aria-label="Add App Server" title="Add App Server" onClick={() => select('app-servers', 'new')}>
                <Plus size={15} aria-hidden="true" />
              </button> : null}
            </div>
            <div className="workspace-session-list fleet-sidebar-items">
              {id === 'app-servers' ? <>
                <FleetSidebarItem label="Local" status={statusFor('local')} active={selection.section === id && selection.itemId === 'local'} onClick={() => select(id, 'local')} />
                {state?.appServers.map((server) => <FleetSidebarItem key={server.id} label={server.name} status={statusFor(server.id)} active={selection.section === id && selection.itemId === server.id} onClick={() => select(id, server.id)} />)}
              </> : null}
              {id === 'browsers' ? state?.browsers.map((browser) => (
                <div className="workspace-session-row fleet-browser-row" key={browser.id}>
                  <span className="workspace-session-item fleet-browser-label" title={browser.name}><span className="workspace-session-title">{browser.name}</span></span>
                  <input
                    type="checkbox"
                    className="fleet-browser-toggle"
                    aria-label={`Enable ${browser.name}`}
                    checked={browser.enabled}
                    disabled={updatingBrowserId !== null}
                    onChange={(event) => void updateBrowser(browser.id, event.currentTarget.checked)}
                  />
                </div>
              )) : null}
              {id === 'machines' ? state?.machines.map((machine) => <FleetSidebarItem key={machine.id} label={machine.name} active={selection.section === id && selection.itemId === machine.id} onClick={() => select(id, machine.id)} />) : null}
              {state && id === 'machines' && state.machines.length === 0 ? <span className="workspace-session-empty">No virtual machines found.</span> : null}
              {state && id === 'browsers' && state.browsers.length === 0 ? <span className="workspace-session-empty">No browsers found.</span> : null}
            </div>
          </div>
        ))}
      </nav>
    </CollectionSidebar>
  );
}

function FleetSidebarItem({ label, status, active, onClick }: { label: string; status?: FleetAppServerStatus; active: boolean; onClick: () => void }): JSX.Element {
  return <div className="workspace-session-row">
    <button type="button" className={`workspace-session-item fleet-sidebar-item${status ? ' fleet-sidebar-server-item' : ''}${active ? ' active' : ''}`} title={status ? `${label}: ${status.message}` : label} aria-label={status ? `${label}, ${status.message}` : label} onClick={onClick}>
      <span className="workspace-session-title">{label}</span>
      {status ? <span className={`sidebar-server-status-dot ${status.state}`} aria-hidden="true" /> : null}
    </button>
  </div>;
}
