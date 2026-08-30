import { describe, expect, it } from 'vitest';
import { needsExplicitRoundedWindowShape, roundedRectShape } from '../src/main/windowShape';

describe('desktop native window shape', () => {
  it('uses explicit clipping for frameless Windows and Linux windows', () => {
    expect(needsExplicitRoundedWindowShape('win32')).toBe(true);
    expect(needsExplicitRoundedWindowShape('linux')).toBe(true);
    expect(needsExplicitRoundedWindowShape('darwin')).toBe(false);
  });

  it('removes the corner pixels while preserving the full center and symmetric edges', () => {
    const shape = roundedRectShape(100, 80, 8);

    expect(shape[0]).toMatchObject({ x: 6, y: 0, width: 88, height: 1 });
    expect(shape).toContainEqual({ x: 0, y: 8, width: 100, height: 64 });
    expect(shape.at(-1)).toMatchObject({ x: 6, y: 79, width: 88, height: 1 });
  });
});
