import { describe, expect, it } from 'vitest';
import type { DesktopUpdateState } from '../src/shared/types';
import { DesktopUpdater, type DesktopUpdaterBackend } from '../src/main/desktopUpdater';
import { desktopUpdateAction } from '../src/renderer/app/TopBar';

class FakeUpdaterBackend implements DesktopUpdaterBackend {
  public autoDownload = true;
  public autoInstallOnAppQuit = true;
  public latestVersion: string | null = '0.9.1';
  public checkFailure: Error | null = null;
  public downloadFailure: Error | null = null;
  public installs: Array<{ isSilent: boolean; isForceRunAfter: boolean }> = [];
  private progressListener: ((progress: { percent: number }) => void) | null = null;

  public async checkForUpdates() {
    if (this.checkFailure) throw this.checkFailure;
    return this.latestVersion
      ? { isUpdateAvailable: true, updateInfo: { version: this.latestVersion } }
      : { isUpdateAvailable: false, updateInfo: { version: '0.9.0' } };
  }

  public async downloadUpdate() {
    this.progressListener?.({ percent: 42.7 });
    if (this.downloadFailure) throw this.downloadFailure;
    return [];
  }

  public quitAndInstall(isSilent: boolean, isForceRunAfter: boolean): void {
    this.installs.push({ isSilent, isForceRunAfter });
  }

  public on(_event: 'download-progress', listener: (progress: { percent: number }) => void): void {
    this.progressListener = listener;
  }
}

function fixture(stopLocalAppServer: () => Promise<void> = async () => undefined) {
  const backend = new FakeUpdaterBackend();
  const states: DesktopUpdateState[] = [];
  const updater = new DesktopUpdater({
    backend,
    currentVersion: '0.9.0',
    stopLocalAppServer,
    onChange: (state) => states.push(state)
  });
  return { backend, states, updater };
}

describe('Desktop updater', () => {
  it('never downloads or installs without an explicit request', () => {
    const { backend } = fixture();
    expect(backend.autoDownload).toBe(false);
    expect(backend.autoInstallOnAppQuit).toBe(false);
  });

  it('moves from check through download to install after stopping the app-server', async () => {
    const order: string[] = [];
    const { backend, states, updater } = fixture(async () => { order.push('stop'); });
    const quitAndInstall = backend.quitAndInstall.bind(backend);
    backend.quitAndInstall = (isSilent, isForceRunAfter) => { order.push('install'); quitAndInstall(isSilent, isForceRunAfter); };

    expect((await updater.check()).status).toBe('available');
    expect((await updater.download()).status).toBe('ready');
    expect(states.map((state) => state.status)).toEqual(['checking', 'available', 'downloading', 'downloading', 'ready']);
    expect(states[3]?.downloadPercent).toBe(42);

    expect((await updater.install()).status).toBe('installing');
    expect(order).toEqual(['stop', 'install']);
    expect(backend.installs).toEqual([{ isSilent: true, isForceRunAfter: true }]);
  });

  it('keeps the update ready and does not restart while research sessions block the app-server stop', async () => {
    const { backend, updater } = fixture(async () => {
      throw new Error('The Beale app-server cannot restart while 1 research session is active.');
    });
    await updater.check();
    await updater.download();

    const state = await updater.install();
    expect(state.status).toBe('ready');
    expect(state.message).toContain('1 research session is active');
    expect(backend.installs).toEqual([]);
  });

  it('offers the download again after it fails', async () => {
    const { backend, updater } = fixture();
    backend.downloadFailure = new Error('connection reset');
    await updater.check();

    const state = await updater.download();
    expect(state).toMatchObject({ status: 'available', availableVersion: '0.9.1', message: 'Download failed: connection reset' });
  });

  it('does not let a periodic check hide a downloaded update', async () => {
    const { backend, updater } = fixture();
    await updater.check();
    await updater.download();
    backend.latestVersion = null;

    expect((await updater.check()).status).toBe('ready');
  });

  it('records a failed check and recovers on the next one', async () => {
    const { backend, updater } = fixture();
    backend.checkFailure = new Error('offline');
    expect(await updater.check()).toMatchObject({ status: 'error', message: 'Update check failed: offline' });

    backend.checkFailure = null;
    backend.latestVersion = null;
    expect(await updater.check()).toMatchObject({ status: 'idle', message: null });
  });
});

describe('Desktop update header action', () => {
  const base: DesktopUpdateState = {
    status: 'idle',
    currentVersion: '0.9.0',
    availableVersion: '0.9.1',
    downloadPercent: null,
    message: null
  };

  it('stays hidden until there is an update to act on', () => {
    for (const status of ['disabled', 'idle', 'checking', 'error'] as const) {
      expect(desktopUpdateAction({ ...base, status })).toBeNull();
    }
  });

  it('labels each actionable step and disables the in-progress ones', () => {
    expect(desktopUpdateAction({ ...base, status: 'available' })).toMatchObject({ label: 'Update to 0.9.1' });
    expect(desktopUpdateAction({ ...base, status: 'downloading', downloadPercent: 42 })).toMatchObject({ label: 'Downloading 42%', run: null });
    expect(desktopUpdateAction({ ...base, status: 'ready' })?.label).toBe('Restart to update');
    expect(desktopUpdateAction({ ...base, status: 'installing' })?.run).toBeNull();
  });

  it('explains a refused restart beside the button', () => {
    const message = 'The Beale app-server cannot restart while 1 research session is active.';
    expect(desktopUpdateAction({ ...base, status: 'ready', message })?.title).toContain(message);
  });
});
