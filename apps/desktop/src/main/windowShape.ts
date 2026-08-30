import type { Rectangle } from 'electron';

export const NATIVE_WINDOW_SHAPE_RADIUS_PX = 8;

export function needsExplicitRoundedWindowShape(platform: NodeJS.Platform): boolean {
  return platform === 'win32' || platform === 'linux';
}

export function roundedRectShape(width: number, height: number, radius: number): Rectangle[] {
  const safeRadius = Math.max(0, Math.min(radius, Math.floor(width / 2), Math.floor(height / 2)));
  if (safeRadius <= 0) return [{ x: 0, y: 0, width, height }];

  const rects: Rectangle[] = [];
  for (let y = 0; y < safeRadius; y += 1) {
    const distanceFromCenter = safeRadius - y - 0.5;
    const inset = Math.ceil(safeRadius - Math.sqrt(Math.max(0, safeRadius * safeRadius - distanceFromCenter * distanceFromCenter)));
    rects.push({ x: inset, y, width: Math.max(0, width - inset * 2), height: 1 });
  }

  const centerHeight = height - safeRadius * 2;
  if (centerHeight > 0) {
    rects.push({ x: 0, y: safeRadius, width, height: centerHeight });
  }

  for (let y = safeRadius - 1; y >= 0; y -= 1) {
    const distanceFromCenter = safeRadius - y - 0.5;
    const inset = Math.ceil(safeRadius - Math.sqrt(Math.max(0, safeRadius * safeRadius - distanceFromCenter * distanceFromCenter)));
    rects.push({ x: inset, y: height - y - 1, width: Math.max(0, width - inset * 2), height: 1 });
  }
  return rects;
}
