import type { JSX, PointerEvent as ReactPointerEvent } from 'react';
import { Monitor, Network, ServerCog } from 'lucide-react';

const sections = [
  { id: 'fleet-configuration-heading', label: 'Fleet', icon: Network },
  { id: 'fleet-app-servers-heading', label: 'App Servers', icon: ServerCog },
  { id: 'fleet-machines-heading', label: 'Virtual Machines', icon: Monitor }
] as const;

export function FleetSidebar({ collapsed, onResizePointerDown }: {
  collapsed: boolean;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
}): JSX.Element {
  return (
    <aside className="sidebar settings-sidebar" aria-hidden={collapsed} inert={collapsed}>
      <div className="sidebar-section settings-sidebar-section">
        <div className="sidebar-wordmark">Fleet</div>
        <nav className="settings-sections" aria-label="Fleet sections">
          {sections.map(({ id, label, icon: Icon }) => (
            <div className="workspace-item-row no-menu" key={id}>
              <button type="button" className="workspace-item" onClick={() => document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })}>
                <Icon size={15} aria-hidden="true" />
                <span>{label}</span>
              </button>
            </div>
          ))}
        </nav>
      </div>
      <div className="sidebar-resize-handle" role="separator" aria-label="Resize sidebar" aria-orientation="vertical" onPointerDown={onResizePointerDown} />
    </aside>
  );
}
