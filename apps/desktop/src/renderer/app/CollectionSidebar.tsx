import type { JSX, PointerEvent as ReactPointerEvent, ReactNode } from 'react';
import { MainSideScrollRegion } from './MainSideScrollRegion';

export function CollectionSidebar({
  title,
  label,
  collapsed,
  error,
  updateKey,
  onResizePointerDown,
  children
}: {
  title: string;
  label: string;
  collapsed: boolean;
  error: string | null;
  updateKey: string;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
  children: ReactNode;
}): JSX.Element {
  return (
    <aside className="sidebar collection-sidebar" aria-label={label} aria-hidden={collapsed} inert={collapsed}>
      <div className="sidebar-section collection-sidebar-section">
        <div className="sidebar-wordmark">{title}</div>
        <MainSideScrollRegion
          className="sidebar-list-scroll-region"
          listClassName="sidebar-list-scroll"
          updateKey={updateKey}
        >
          {children}
        </MainSideScrollRegion>
      </div>
      {error ? <div className="error-box">{error}</div> : null}
      <div className="sidebar-resize-handle" role="separator" aria-label="Resize sidebar" aria-orientation="vertical" onPointerDown={onResizePointerDown} />
    </aside>
  );
}
