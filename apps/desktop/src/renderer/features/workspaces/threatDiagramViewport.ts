export interface DiagramView {
  scale: number;
  x: number;
  y: number;
}

export interface DiagramViewport {
  width: number;
  height: number;
  contentWidth: number;
  contentHeight: number;
}

export const INITIAL_DIAGRAM_VIEW: DiagramView = { scale: 1, x: 0, y: 0 };
export const MAX_DIAGRAM_SCALE = 6;

export function clampDiagramView(view: DiagramView, viewport: DiagramViewport): DiagramView {
  const scale = Math.min(MAX_DIAGRAM_SCALE, Math.max(1, view.scale));
  const maxX = Math.max(0, (viewport.contentWidth * scale - viewport.width) / 2);
  const maxY = Math.max(0, (viewport.contentHeight * scale - viewport.height) / 2);
  return {
    scale,
    x: Math.min(maxX, Math.max(-maxX, view.x)),
    y: Math.min(maxY, Math.max(-maxY, view.y))
  };
}

export function zoomDiagramView(view: DiagramView, nextScale: number, anchorX: number, anchorY: number,
  viewport: DiagramViewport): DiagramView {
  const scale = Math.min(MAX_DIAGRAM_SCALE, Math.max(1, nextScale));
  const ratio = scale / view.scale;
  const centerX = viewport.width / 2;
  const centerY = viewport.height / 2;
  return clampDiagramView({
    scale,
    x: anchorX - centerX - (anchorX - centerX - view.x) * ratio,
    y: anchorY - centerY - (anchorY - centerY - view.y) * ratio
  }, viewport);
}
