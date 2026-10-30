import { useEffect, useRef, useState } from 'react';
import type { JSX, KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Check, ChevronDown, Computer } from 'lucide-react';
import type { FleetAppServer, FleetAppServerStatus } from '@beale/app-server-runtime/protocol';

export function SidebarServerSelector({ servers, statuses, value, collapsed, onChange, onAdd, onRefreshStatuses }: {
  servers: FleetAppServer[];
  statuses: Record<string, FleetAppServerStatus>;
  value: string;
  collapsed: boolean;
  onChange: (serverId: string) => void;
  onAdd: () => void;
  onRefreshStatuses: () => void;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const pickerRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const options = [{ id: 'local', name: 'Local' }, ...servers];
  const selectedIndex = Math.max(0, options.findIndex((option) => option.id === value));
  const selected = options[selectedIndex]!;
  const statusFor = (id: string): FleetAppServerStatus => statuses[id] ?? { id, state: 'offline', message: 'Status unavailable.' };

  useEffect(() => {
    if (collapsed) setOpen(false);
  }, [collapsed]);

  useEffect(() => {
    if (!open) return undefined;
    optionRefs.current[selectedIndex]?.focus();
    const dismissOnOutsidePointer = (event: PointerEvent): void => {
      if (!pickerRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', dismissOnOutsidePointer);
    return () => document.removeEventListener('pointerdown', dismissOnOutsidePointer);
  }, [open, selectedIndex]);

  const handleMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    if (event.key === 'Escape') {
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
      return;
    }
    const currentIndex = optionRefs.current.findIndex((option) => option === document.activeElement);
    const itemCount = options.length + 1;
    const nextIndex = event.key === 'ArrowDown' ? (currentIndex + 1) % itemCount
      : event.key === 'ArrowUp' ? (currentIndex - 1 + itemCount) % itemCount
        : event.key === 'Home' ? 0
          : event.key === 'End' ? itemCount - 1 : null;
    if (nextIndex === null) return;
    event.preventDefault();
    optionRefs.current[nextIndex]?.focus();
  };

  return (
    <div
      ref={pickerRef}
      className="sidebar-server-picker"
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
    >
      <button
        ref={triggerRef}
        type="button"
        className="sidebar-server-selector"
        title={selected.name}
        aria-label={`App server: ${selected.name}`}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => { if (!open) onRefreshStatuses(); setOpen(!open); }}
        onKeyDown={(event) => {
          if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
          event.preventDefault();
          if (!open) onRefreshStatuses();
          setOpen(true);
        }}
      >
        <Computer size={14} aria-hidden="true" />
        <span className="sidebar-server-selector-label">{selected.name}</span>
        <ChevronDown size={12} aria-hidden="true" />
      </button>
      {open ? (
        <div className="sidebar-server-menu" role="menu" aria-label="App server" onKeyDown={handleMenuKeyDown}>
          {options.map((option, index) => {
            const status = statusFor(option.id);
            return (
              <button
                key={option.id}
                ref={(element) => { optionRefs.current[index] = element; }}
                type="button"
                role="menuitemradio"
                aria-checked={option.id === selected.id}
                tabIndex={index === selectedIndex ? 0 : -1}
                title={`${option.name}: ${status.message}`}
                aria-label={`${option.name}, ${status.message}`}
                onClick={() => {
                  setOpen(false);
                  triggerRef.current?.focus();
                  if (option.id !== value) onChange(option.id);
                }}
              >
                <span className="sidebar-server-option-main"><span className={`sidebar-server-status-dot ${status.state}`} aria-hidden="true" /><span className="sidebar-server-option-name">{option.name}</span></span>
                {option.id === selected.id ? <Check size={13} aria-hidden="true" /> : null}
              </button>
            );
          })}
          <div className="sidebar-server-menu-add" role="none">
            <button
              ref={(element) => { optionRefs.current[options.length] = element; }}
              type="button"
              role="menuitem"
              tabIndex={-1}
              onClick={() => { setOpen(false); triggerRef.current?.focus(); onAdd(); }}
            >Add App Server</button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
