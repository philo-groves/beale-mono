import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  APP_SERVER_LOCAL_PORT,
  APP_SERVER_TAILSCALE_HTTPS_PORT,
  appServerRemoteAccessLaunchEnvironment,
  detectAppServerMagicDnsName,
  readAppServerRemoteAccessSettings,
  updateAppServerRemoteAccess,
  type TailscaleCommandRunner
} from '../src/main/appServerRemoteAccess';

let directory = '';
let originalConfigPath: string | undefined;

beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), 'beale-app-server-remote-access-'));
  originalConfigPath = process.env.BEALE_APP_SERVER_REMOTE_ACCESS_FILE;
  process.env.BEALE_APP_SERVER_REMOTE_ACCESS_FILE = join(directory, 'remote-access.json');
});

afterEach(() => {
  if (originalConfigPath === undefined) delete process.env.BEALE_APP_SERVER_REMOTE_ACCESS_FILE;
  else process.env.BEALE_APP_SERVER_REMOTE_ACCESS_FILE = originalConfigPath;
  rmSync(directory, { recursive: true, force: true });
});

describe('app-server remote access', () => {
  it('defaults disabled and detects the machine MagicDNS name', async () => {
    expect(readAppServerRemoteAccessSettings()).toMatchObject({
      enabled: false,
      magicDnsName: '',
      publicUrl: null,
      status: 'disabled'
    });
    const runner: TailscaleCommandRunner = async (args) => {
      expect(args).toEqual(['status', '--json']);
      return JSON.stringify({ BackendState: 'Running', Self: { DNSName: 'Mac-Name.Tailnet.ts.net.' } });
    };
    await expect(detectAppServerMagicDnsName(runner)).resolves.toMatchObject({
      enabled: false,
      magicDnsName: 'mac-name.tailnet.ts.net',
      publicUrl: `https://mac-name.tailnet.ts.net:${APP_SERVER_TAILSCALE_HTTPS_PORT}`,
      status: 'available'
    });
  });

  it('configures a dedicated HTTPS Serve port and supplies app-server launch variables', async () => {
    const calls: string[][] = [];
    const runner: TailscaleCommandRunner = async (args) => {
      calls.push(args);
      return '';
    };
    const settings = await updateAppServerRemoteAccess({
      enabled: true,
      magicDnsName: 'beale-mac.example.ts.net'
    }, runner);

    expect(calls).toEqual([[
      'serve',
      '--bg',
      '--yes',
      `--https=${APP_SERVER_TAILSCALE_HTTPS_PORT}`,
      `http://127.0.0.1:${APP_SERVER_LOCAL_PORT}`
    ]]);
    expect(settings).toMatchObject({
      enabled: true,
      publicUrl: `https://beale-mac.example.ts.net:${APP_SERVER_TAILSCALE_HTTPS_PORT}`,
      status: 'configured'
    });
    expect(appServerRemoteAccessLaunchEnvironment()).toEqual({
      BEALE_APP_SERVER_HOST: '127.0.0.1',
      BEALE_APP_SERVER_PORT: String(APP_SERVER_LOCAL_PORT),
      BEALE_APP_SERVER_PUBLIC_URL: `https://beale-mac.example.ts.net:${APP_SERVER_TAILSCALE_HTTPS_PORT}`
    });
    expect(JSON.parse(readFileSync(process.env.BEALE_APP_SERVER_REMOTE_ACCESS_FILE!, 'utf8'))).toMatchObject({
      version: 1,
      enabled: true,
      magicDnsName: 'beale-mac.example.ts.net'
    });
  });

  it('removes only Beale dedicated Serve listener when disabled', async () => {
    const calls: string[][] = [];
    const runner: TailscaleCommandRunner = async (args) => {
      calls.push(args);
      return '';
    };
    await updateAppServerRemoteAccess({ enabled: true, magicDnsName: 'beale.example.ts.net' }, runner);
    const disabled = await updateAppServerRemoteAccess({ enabled: false }, runner);
    expect(calls.at(-1)).toEqual(['serve', `--https=${APP_SERVER_TAILSCALE_HTTPS_PORT}`, 'off']);
    expect(disabled).toMatchObject({ enabled: false, status: 'disabled' });
    expect(appServerRemoteAccessLaunchEnvironment()).toEqual({});
  });

  it('rejects non-MagicDNS remote hosts before changing Serve', async () => {
    const runner: TailscaleCommandRunner = async () => {
      throw new Error('should not run');
    };
    await expect(updateAppServerRemoteAccess({ enabled: true, magicDnsName: 'example.com' }, runner))
      .rejects.toThrow(/ending in \.ts\.net/);
  });
});
