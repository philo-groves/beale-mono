import type { DesktopUpdateState } from '@shared/types';

/**
 * The subset of electron-updater's AppUpdater the controller drives. Tests
 * supply an in-memory implementation; index.ts passes `autoUpdater`.
 */
export interface DesktopUpdaterBackend {
  autoDownload: boolean;
  autoInstallOnAppQuit: boolean;
  checkForUpdates(): Promise<{ isUpdateAvailable: boolean; updateInfo: { version: string } } | null>;
  downloadUpdate(): Promise<unknown>;
  quitAndInstall(isSilent: boolean, isForceRunAfter: boolean): void;
  on(event: 'download-progress', listener: (progress: { percent: number }) => void): unknown;
}

export interface DesktopUpdaterDependencies {
  backend: DesktopUpdaterBackend;
  currentVersion: string;
  /**
   * Stops the local app-server before the installer replaces its files. It
   * must reject, leaving the server running, while research sessions are active.
   */
  stopLocalAppServer(): Promise<void>;
  onChange(state: DesktopUpdateState): void;
}

const IDLE: Omit<DesktopUpdateState, 'currentVersion'> = {
  status: 'idle',
  availableVersion: null,
  downloadPercent: null,
  message: null
};

/**
 * Owns the check -> download -> install sequence for installed builds. Each
 * step is user-initiated except the periodic check: downloads never start on
 * their own, and the app restarts only from install(), after the local
 * app-server has stopped cleanly.
 */
export class DesktopUpdater {
  private current: DesktopUpdateState;

  public constructor(private readonly dependencies: DesktopUpdaterDependencies) {
    this.current = { ...IDLE, currentVersion: dependencies.currentVersion };
    dependencies.backend.autoDownload = false;
    // Installing on quit would replace app-server files while that detached
    // process may still be running from them.
    dependencies.backend.autoInstallOnAppQuit = false;
    dependencies.backend.on('download-progress', (progress) => {
      const percent = Math.floor(progress.percent);
      // Progress events arrive many times per percent; forward only changes.
      if (this.current.status !== 'downloading' || percent === this.current.downloadPercent) return;
      this.set({ downloadPercent: percent });
    });
  }

  public state(): DesktopUpdateState {
    return this.current;
  }

  public async check(): Promise<DesktopUpdateState> {
    // A found or downloaded update stays put; re-checking would hide the button.
    if (this.current.status !== 'idle' && this.current.status !== 'error') return this.current;
    this.set({ ...IDLE, status: 'checking' });
    try {
      const result = await this.dependencies.backend.checkForUpdates();
      if (result?.isUpdateAvailable) {
        this.set({ status: 'available', availableVersion: result.updateInfo.version });
      } else {
        this.set({ ...IDLE });
      }
    } catch (error) {
      this.set({ ...IDLE, status: 'error', message: `Update check failed: ${errorMessage(error)}` });
    }
    return this.current;
  }

  public async download(): Promise<DesktopUpdateState> {
    if (this.current.status !== 'available') return this.current;
    this.set({ status: 'downloading', downloadPercent: 0, message: null });
    try {
      await this.dependencies.backend.downloadUpdate();
      this.set({ status: 'ready', downloadPercent: null });
    } catch (error) {
      // Return to `available` so the button offers the download again.
      this.set({ status: 'available', downloadPercent: null, message: `Download failed: ${errorMessage(error)}` });
    }
    return this.current;
  }

  public async install(): Promise<DesktopUpdateState> {
    if (this.current.status !== 'ready') return this.current;
    this.set({ status: 'installing', message: null });
    try {
      await this.dependencies.stopLocalAppServer();
    } catch (error) {
      this.set({ status: 'ready', message: errorMessage(error) });
      return this.current;
    }
    // Silent install with forced relaunch: one click goes straight back into
    // Beale on the new version instead of through an installer wizard.
    this.dependencies.backend.quitAndInstall(true, true);
    return this.current;
  }

  private set(patch: Partial<DesktopUpdateState>): void {
    this.current = { ...this.current, ...patch };
    this.dependencies.onChange(this.current);
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
