import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const projectRoot = process.cwd();

describe('desktop app branding', () => {
  it('brands every supported source-launch path before Electron starts', () => {
    const packageJson = JSON.parse(readFileSync(join(projectRoot, 'package.json'), 'utf8')) as {
      productName: string;
      scripts: Record<string, string>;
    };

    expect(packageJson.productName).toBe('Beale');
    expect(packageJson.scripts.postinstall).toBe('pnpm run brand:electron');
    expect(packageJson.scripts.prestart).toBe('pnpm run brand:electron');
    expect(packageJson.scripts.predev).toBe('pnpm run brand:electron');
    expect(packageJson.scripts.prepreview).toBe('pnpm run brand:electron');

    const appServerPackageJson = JSON.parse(readFileSync(join(projectRoot, '..', '..', 'app-server', 'package.json'), 'utf8')) as {
      main: string;
      scripts: Record<string, string>;
    };
    expect(appServerPackageJson.main).toBe('./dist/trayBootstrap.js');
    expect(appServerPackageJson.scripts.prestart).toBe('node ../apps/desktop/scripts/brand-electron.mjs');
  });

  it.runIf(process.platform === 'darwin')('embeds the Beale identity and icon in the macOS Electron bundle', () => {
    execFileSync(process.execPath, [join(projectRoot, 'scripts/brand-electron.mjs')]);
    const contentsPath = join(projectRoot, 'node_modules/electron/dist/Electron.app/Contents');
    const plistPath = join(contentsPath, 'Info.plist');
    const plistValue = (key: string): string => execFileSync(
      'plutil',
      ['-extract', key, 'raw', plistPath],
      { encoding: 'utf8' }
    ).trim();

    expect(plistValue('CFBundleDisplayName')).toBe('Beale');
    expect(plistValue('CFBundleName')).toBe('Beale');
    expect(plistValue('CFBundleIdentifier')).toBe('com.beale.app');
    expect(plistValue('CFBundleIconFile')).toBe('beale.icns');
    expect(plistValue('NSLocalNetworkUsageDescription')).toBe(
      'Beale connects to authorized local virtual machines and research targets.'
    );
    expect(() => plistValue('NSScreenCaptureUsageDescription')).toThrow();
    expect(readFileSync(join(contentsPath, 'Resources/beale.icns')).byteLength).toBeGreaterThan(0);
  });

  it('launches the macOS tray host through LaunchServices instead of its anonymous inner executable', () => {
    const source = readFileSync(join(projectRoot, 'src/main/bealeAppServerClient.ts'), 'utf8');
    expect(source).toContain("command: '/usr/bin/open'");
    expect(source).toContain("args: ['-n', '-g', '--stderr', '/dev/stderr', '-a', electronRuntime, '--args', appServerRoot]");
    expect(source).toContain("APP_SERVER_LAUNCH_ENVIRONMENT_FLAG");
  });
});
