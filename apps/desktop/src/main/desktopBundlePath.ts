import { basename, dirname, join } from 'node:path';

/** Electron's app path is the app root when packaged, but out/main when started by file. */
export function desktopBundlePath(appPath: string, ...segments: string[]): string {
  const outputRoot = basename(appPath) === 'main' && basename(dirname(appPath)) === 'out'
    ? dirname(appPath)
    : join(appPath, 'out');
  return join(outputRoot, ...segments);
}
