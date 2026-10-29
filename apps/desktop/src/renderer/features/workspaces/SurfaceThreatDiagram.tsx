import { useCallback, useEffect, useId, useRef, useState, type JSX } from 'react';
import { createPortal } from 'react-dom';
import { Maximize2, Minus, Plus, RotateCcw } from 'lucide-react';
import { Modal } from '../../app/Modal';
import { clampDiagramView, INITIAL_DIAGRAM_VIEW, MAX_DIAGRAM_SCALE, zoomDiagramView,
  type DiagramViewport } from './threatDiagramViewport';
import { surfaceThreatDiagramConfig, type SurfaceDiagramTheme } from './surfaceThreatDiagramTheme';
import { normalizeMermaidSvg } from './normalizeMermaidSvg';

let nextRenderId = 0;

function currentTheme(): SurfaceDiagramTheme {
  if (typeof document === 'undefined') return 'dark';
  const theme = document.documentElement.dataset.theme;
  return theme === 'light' || theme === 'cream' || theme === 'midnight' ? theme : 'dark';
}

function ExpandedThreatDiagram({ url, surfaceName }: { url: string; surfaceName: string }): JSX.Element {
  const viewportRef = useRef<HTMLDivElement>(null);
  const imageRef = useRef<HTMLImageElement>(null);
  const dragRef = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  const [view, setView] = useState(INITIAL_DIAGRAM_VIEW);
  const [dragging, setDragging] = useState(false);

  const viewport = useCallback((): DiagramViewport | null => {
    const container = viewportRef.current;
    const image = imageRef.current;
    if (!container || !image) return null;
    const { width, height } = container.getBoundingClientRect();
    if (!width || !height) return null;
    const aspect = image.naturalWidth && image.naturalHeight ? image.naturalWidth / image.naturalHeight : width / height;
    return { width, height, contentWidth: Math.min(width, height * aspect),
      contentHeight: Math.min(height, width / aspect) };
  }, []);

  const zoom = useCallback((factor: number, anchorX?: number, anchorY?: number): void => {
    const size = viewport();
    if (!size) return;
    setView((current) => zoomDiagramView(current, current.scale * factor,
      anchorX ?? size.width / 2, anchorY ?? size.height / 2, size));
  }, [viewport]);

  useEffect(() => {
    const container = viewportRef.current;
    if (!container) return;
    const onWheel = (event: WheelEvent): void => {
      event.preventDefault();
      const rect = container.getBoundingClientRect();
      zoom(Math.exp(-event.deltaY * 0.0015), event.clientX - rect.left, event.clientY - rect.top);
    };
    container.addEventListener('wheel', onWheel, { passive: false });
    return () => container.removeEventListener('wheel', onWheel);
  }, [zoom]);

  useEffect(() => setView(INITIAL_DIAGRAM_VIEW), [url]);

  return <div ref={viewportRef} className="workspace-surface-threat-viewer" data-dragging={dragging} data-zoomed={view.scale > 1}
    onPointerDown={(event) => {
      if (view.scale <= 1 || event.button !== 0 || event.target !== event.currentTarget) return;
      event.currentTarget.setPointerCapture(event.pointerId);
      dragRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY };
      setDragging(true);
    }}
    onPointerMove={(event) => {
      const drag = dragRef.current;
      const size = viewport();
      if (!drag || drag.pointerId !== event.pointerId || !size) return;
      const dx = event.clientX - drag.x;
      const dy = event.clientY - drag.y;
      dragRef.current = { ...drag, x: event.clientX, y: event.clientY };
      setView((current) => clampDiagramView({ ...current, x: current.x + dx, y: current.y + dy }, size));
    }}
    onPointerUp={(event) => {
      if (dragRef.current?.pointerId !== event.pointerId) return;
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
      dragRef.current = null;
      setDragging(false);
    }}
    onPointerCancel={(event) => {
      if (dragRef.current?.pointerId !== event.pointerId) return;
      dragRef.current = null;
      setDragging(false);
    }}
    onLostPointerCapture={(event) => {
      if (dragRef.current?.pointerId !== event.pointerId) return;
      dragRef.current = null;
      setDragging(false);
    }}>
    <img ref={imageRef} src={url} alt={`Threat model diagram for ${surfaceName}`}
      onLoad={() => {
        const size = viewport();
        if (size) setView((current) => clampDiagramView(current, size));
      }}
      style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})` }} />
    <div className="workspace-surface-threat-viewer-controls" role="group" aria-label="Diagram zoom controls">
      <button type="button" aria-label="Zoom out" title="Zoom out" disabled={view.scale <= 1} onClick={() => zoom(1 / 1.25)}>
        <Minus aria-hidden="true" size={16} />
      </button>
      <span aria-live="polite">{Math.round(view.scale * 100)}%</span>
      <button type="button" aria-label="Zoom in" title="Zoom in" disabled={view.scale >= MAX_DIAGRAM_SCALE} onClick={() => zoom(1.25)}>
        <Plus aria-hidden="true" size={16} />
      </button>
      <button type="button" aria-label="Reset diagram zoom" title="Reset zoom" disabled={view.scale === 1}
        onClick={() => setView(INITIAL_DIAGRAM_VIEW)}>
        <RotateCcw aria-hidden="true" size={15} />
      </button>
    </div>
  </div>;
}

export function SurfaceThreatDiagram({ source, surfaceName }: {
  source: string; surfaceName: string;
}): JSX.Element {
  const id = `surface-threat-${useId().replace(/[^a-zA-Z0-9_-]/gu, '')}`;
  const [theme, setTheme] = useState(currentTheme);
  const [rendered, setRendered] = useState<{ key: string; url?: string; error?: boolean } | null>(null);
  const [expanded, setExpanded] = useState(false);
  const expandButtonRef = useRef<HTMLButtonElement>(null);
  const key = `${theme}\0${source}`;

  useEffect(() => {
    const observer = new MutationObserver(() => setTheme(currentTheme()));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!source.trim()) return;
    let disposed = false;
    let objectUrl: string | null = null;
    void import('mermaid').then(async ({ default: mermaid }) => {
      mermaid.initialize(surfaceThreatDiagramConfig(theme));
      const { svg } = await mermaid.render(`${id}-${++nextRenderId}`, source);
      if (disposed) return;
      objectUrl = URL.createObjectURL(new Blob([normalizeMermaidSvg(svg)], { type: 'image/svg+xml' }));
      setRendered({ key, url: objectUrl });
    }).catch(() => {
      if (!disposed) setRendered({ key, error: true });
    });
    return () => {
      disposed = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [id, key, source, theme]);

  useEffect(() => {
    if (!expanded) return;
    const closeOnEscape = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setExpanded(false);
    };
    document.addEventListener('keydown', closeOnEscape);
    document.querySelector<HTMLButtonElement>('.workspace-surface-threat-diagram-modal .modal-header button')?.focus();
    return () => {
      document.removeEventListener('keydown', closeOnEscape);
      expandButtonRef.current?.focus();
    };
  }, [expanded]);

  if (!source.trim()) return <p className="workspace-surface-details-muted">No threat model diagram recorded.</p>;
  const url = rendered?.key === key ? rendered.url : undefined;
  const error = rendered?.key === key && rendered.error;
  const diagram = url ? <img src={url} alt={`Threat model diagram for ${surfaceName}`}
    onError={() => setRendered({ key, error: true })} />
    : error ? <div className="workspace-surface-threat-diagram-error" role="status">
      <p>Could not render this Mermaid diagram.</p>
      <pre>{source}</pre>
    </div>
      : <p className="workspace-surface-details-muted" role="status">Rendering diagram…</p>;

  return <>
    <div className="workspace-surface-threat-diagram">
      {diagram}
      <button ref={expandButtonRef} className="workspace-surface-threat-diagram-expand" type="button"
        aria-label={`Expand threat model diagram for ${surfaceName}`} title="Expand diagram" onClick={() => setExpanded(true)}>
        <Maximize2 aria-hidden="true" size={16} />
      </button>
    </div>
    {expanded ? createPortal(<Modal className="workspace-surface-threat-diagram-modal"
      title={`${surfaceName} threat model diagram`} onClose={() => setExpanded(false)}>
      {url ? <ExpandedThreatDiagram url={url} surfaceName={surfaceName} /> : diagram}
    </Modal>, document.body) : null}
  </>;
}
