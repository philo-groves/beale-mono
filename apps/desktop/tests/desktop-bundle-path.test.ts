import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { desktopBundlePath } from '../src/main/desktopBundlePath';

describe('Desktop bundle paths', () => {
  const appRoot = join('C:', 'example', 'Beale');
  const sourceEntryRoot = join(appRoot, 'out', 'main');

  it('loads renderer and preload from the packaged app root', () => {
    expect(desktopBundlePath(appRoot, 'renderer', 'index.html'))
      .toBe(join(appRoot, 'out', 'renderer', 'index.html'));
    expect(desktopBundlePath(appRoot, 'preload', 'index.js'))
      .toBe(join(appRoot, 'out', 'preload', 'index.js'));
  });

  it('loads sibling bundles when Electron starts from out/main/index.js', () => {
    expect(desktopBundlePath(sourceEntryRoot, 'renderer', 'index.html'))
      .toBe(join(appRoot, 'out', 'renderer', 'index.html'));
    expect(desktopBundlePath(sourceEntryRoot, 'preload', 'index.js'))
      .toBe(join(appRoot, 'out', 'preload', 'index.js'));
  });
});
