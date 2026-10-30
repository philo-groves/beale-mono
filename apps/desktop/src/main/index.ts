import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, safeStorage, session, shell } from 'electron';
import { installPreBealeEnvironmentAliases } from '@beale/research-agent/legacy-compatibility';
import type { IpcMainInvokeEvent, MessageBoxOptions } from 'electron';
import { appendFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { installUndiciTypeOfServiceCompatibility } from '@beale/app-server-runtime/node-network-compatibility';
import { BEALE_APP_SERVER_CONTRACT_TIMESTAMP, decodeGeneratedResearchGoalSuggestions } from '@beale/app-server-runtime/protocol';
import { IPC_CHANNELS } from '@shared/ipc';
import { runDetailProjectionMetricLabel } from '../shared/runDetailProjection';
import type {
  AppServerMemoryDirectorySummary,
  AutomationUpdateInput,
  AppServerRemoteAccessUpdate,
  AppServerToolingConfigUpdate,
  NativeMenuAction,
  ProfilingReport,
  WorkspaceRegistryState,
  WorkspaceOnboardingInput,
  WorkspaceOnboardingSkipInput,
  WorkspaceScopeDraft,
  SteeringSuggestionResult,
  ResearchGoalSuggestionInput,
  ResearchGoalSuggestionSelectionInput,
  ResearchPromptGenerationInput,
  ResearchKitRefreshInput,
  ResearchProviderId,
  QuickChatStartInput,
  ResearchModelProviderId,
  ResearchProfileId,
  ProviderModelDefaults,
  ProviderAuthenticationMethod,
  ProviderContextSize,
  AppServerReportLocator,
  ReportContentUpdateInput,
  ReportTriageStatusUpdateInput,
  ReportSessionStartInput,
  RunDetailUpdateCursor,
  RunDetailProjection,
  RunMessageDetailRequest,
  SessionTranscriptSearchInput,
  MemoryTypeDescriptions,
  MarkHistoryDuplicateInput,
  UndoHistoryDuplicateInput,
  ClaimBoardTransitionRequest,
  ShellOptions,
  StartRunInput,
  SteeringAction,
  WorkspaceSnapshot,
  WorkspacePickerMode,
  WorkspaceEditorId,
  WorkspaceMemoryBackendId,
  RepositoryCloneMode,
  ResearchSessionSummary,
  DesktopUpdateState,
} from '@shared/types';
import { getHostEnvironment, WorkspaceService, type WorkspaceChange } from './workspaceService';
import { nativeMacApplicationMenuTemplate } from './nativeApplicationMenu';
import {
  ProviderCredentialStore,
  unlockProviderApiKeysForAppServerStartup
} from './providerCredentialStore';
import { restoreAndFocusWindow } from './windowLifecycle';
import { desktopBundlePath } from './desktopBundlePath';
import { rendererRecoveryDecision } from './rendererRecovery';
import { DesktopRestartPromptController } from './desktopRestartPrompt';
import { autoUpdater } from 'electron-updater';
import { DesktopUpdater } from './desktopUpdater';
import { IosDeviceCaptureService } from './iosDeviceCaptureService';
import { getWorkspaceEditorCatalogForHost, openWorkspaceInEditor } from './workspaceEditors';
import { resolveContentLink } from './contentLinks';
import { allowedBrowserUrl, InAgentBrowserBridge } from './inAgentBrowserBridge';
import { FleetBrowserViewer } from './fleetBrowserViewer';
import type { FleetBrowserInput } from '../shared/fleetBrowser';
import {
  NATIVE_WINDOW_SHAPE_RADIUS_PX,
  needsExplicitRoundedWindowShape,
  roundedRectShape
} from './windowShape';
import {
  BealeDesktopRestartRequiredError,
  ensureBealeAppServerRunning,
  fetchAppServerCanonicalResultWithRecovery,
  invokeAppServerOperation,
  inspectLocalAppServerStatus,
  restartBealeAppServer,
  restartBealeAppServerViaSupervisor,
  setBealeDesktopRestartRequiredHandler,
  stopBealeAppServer,
  stopBealeAppServerForUpdate,
  testLocalAppServerConnection
} from './bealeAppServerClient';
import {
  detectAppServerMagicDnsName,
  readAppServerRemoteAccessSettings,
  updateAppServerRemoteAccess
} from './appServerRemoteAccess';

installPreBealeEnvironmentAliases();
installUndiciTypeOfServiceCompatibility();

const APP_NAME = 'Beale';
let mainWindow: BrowserWindow | null = null;
let workspaceService: WorkspaceService;
let iosDeviceCaptureService: IosDeviceCaptureService;
let inAgentBrowserBridge: InAgentBrowserBridge | null = null;
const fleetBrowserViewer = new FleetBrowserViewer((update) => {
  if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(IPC_CHANNELS.fleetBrowserUpdate, update);
});
const runDetailRequestControllers = new Map<string, AbortController>();
const researchGoalSuggestionControllers = new Map<string, AbortController>();
const smokeTestMode = process.argv.includes('--smoke-test');
// Installed builds check the GitHub release feed shortly after launch and then
// every few hours. Source checkouts and smoke tests never contact the feed.
const DESKTOP_UPDATE_FIRST_CHECK_DELAY_MS = 10_000;
const DESKTOP_UPDATE_CHECK_INTERVAL_MS = 4 * 60 * 60 * 1000;
let desktopUpdater: DesktopUpdater | null = null;

// Keep the established storage location stable while correcting the displayed app identity.
app.setPath('userData', join(app.getPath('appData'), 'beale'));
app.setName(APP_NAME);
process.title = APP_NAME;

const desktopRestartAttemptPath = (): string => join(app.getPath('userData'), 'desktop-restart-attempt');
const desktopRestartPrompt = new DesktopRestartPromptController({
  showDialog: async (restartAvailable) => {
    const options: MessageBoxOptions = restartAvailable ? {
      type: 'warning',
      title: 'Restart Beale',
      message: 'Beale needs to restart to finish updating.',
      detail: 'The running app-server uses a newer control contract than this Beale process. Restart Beale to load the matching desktop code.',
      buttons: ['Restart Beale', 'Not Now'],
      defaultId: 0,
      cancelId: 1,
      noLink: true
    } : {
      type: 'warning',
      title: 'Update Beale',
      message: 'This Beale build is older than the app-server.',
      detail: 'Restarting loaded the same Desktop build. Install or rebuild a matching Beale version, then reopen it.',
      buttons: ['OK'],
      defaultId: 0,
      cancelId: 0,
      noLink: true
    };
    const owner = mainWindow && !mainWindow.isDestroyed() ? mainWindow : BrowserWindow.getFocusedWindow();
    const result = owner ? await dialog.showMessageBox(owner, options) : await dialog.showMessageBox(options);
    return restartAvailable && result.response === 0 ? 'restart' : 'later';
  },
  wasRestartAttempted: () => {
    try { return readFileSync(desktopRestartAttemptPath(), 'utf8').trim() === BEALE_APP_SERVER_CONTRACT_TIMESTAMP; }
    catch { return false; }
  },
  markRestartAttempt: () => {
    mkdirSync(app.getPath('userData'), { recursive: true });
    writeFileSync(desktopRestartAttemptPath(), `${BEALE_APP_SERVER_CONTRACT_TIMESTAMP}\n`, { mode: 0o600 });
  },
  clearRestartAttempt: () => { rmSync(desktopRestartAttemptPath(), { force: true }); },
  relaunch: () => { app.relaunch(); },
  exit: () => { app.exit(0); },
  reportFailure: (error) => {
    dialog.showErrorBox('Beale restart failed', error instanceof Error ? error.message : String(error));
  }
});

const hasSingleInstanceLock = app.requestSingleInstanceLock();

function normalizedRunDetailProjection(value: unknown): RunDetailProjection {
  if (value === 'commentary') return value;
  if (!value || typeof value !== 'object' || Array.isArray(value)) return 'full';
  const candidate = value as Record<string, unknown>;
  if (candidate.mode !== 'commentary') return 'full';
  if (candidate.agentPath === null) return { mode: 'commentary', agentPath: null };
  if (typeof candidate.agentPath !== 'string') return 'full';
  const agentPath = candidate.agentPath.trim();
  return agentPath.startsWith('/root/') && agentPath.length <= 512
    ? { mode: 'commentary', agentPath }
    : 'full';
}

function createWindow(): void {
  const isMac = process.platform === 'darwin';
  const needsExplicitWindowShape = needsExplicitRoundedWindowShape(process.platform);
  const supportsNativeRoundedCorners = process.platform === 'darwin' || process.platform === 'win32';
  const appIcon = createAppIcon();
  const window = new BrowserWindow({
    width: 1440,
    height: 940,
    minWidth: 1120,
    minHeight: 760,
    show: false,
    title: APP_NAME,
    backgroundColor: '#00000000',
    autoHideMenuBar: true,
    transparent: true,
    hasShadow: isMac,
    roundedCorners: supportsNativeRoundedCorners,
    ...(appIcon ? { icon: appIcon } : {}),
    ...(isMac
      ? {
          titleBarStyle: 'hiddenInset' as const,
          trafficLightPosition: { x: 16, y: 16 }
        }
      : {
          frame: false
        }),
    webPreferences: {
      preload: desktopBundlePath(app.getAppPath(), 'preload', 'index.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webviewTag: true
    }
  });
  mainWindow = window;
  let recentRendererExits: number[] = [];
  let recoveryTimer: ReturnType<typeof setTimeout> | null = null;
  window.webContents.on('render-process-gone', (_event, details) => {
    if (details.reason === 'clean-exit') return;
    recordRendererExit(details.reason, details.exitCode);
    const decision = rendererRecoveryDecision(recentRendererExits, Date.now());
    recentRendererExits = decision.recentExits;
    if (recoveryTimer) clearTimeout(recoveryTimer);
    if (decision.reloadAfterMs === null) {
      void dialog.showMessageBox(window, {
        type: 'error',
        title: 'Beale window stopped',
        message: 'The Beale window stopped repeatedly.',
        detail: 'Restart Beale to reopen the window. Research sessions remain on the app-server.',
        buttons: ['OK']
      });
      return;
    }
    recoveryTimer = setTimeout(() => {
      recoveryTimer = null;
      if (!window.isDestroyed() && mainWindow === window) window.webContents.reload();
    }, decision.reloadAfterMs);
  });
  window.on('closed', () => {
    if (recoveryTimer) clearTimeout(recoveryTimer);
    if (mainWindow === window) mainWindow = null;
  });
  window.setBackgroundColor('#00000000');
  window.setMenuBarVisibility(false);
  registerRoundedWindowShape(window, needsExplicitWindowShape);
  registerWindowStartupShow(window, needsExplicitWindowShape);
  registerWindowChromeStateEvents(window);
  registerRendererDevToolsControls(window);
  registerRendererNavigationPolicy(window);
  window.webContents.on('will-attach-webview', (event, webPreferences, params) => {
    if (!inAgentBrowserBridge?.contextIdForPartition(params.partition) || !allowedBrowserUrl(params.src)) {
      event.preventDefault();
      return;
    }
    delete webPreferences.preload;
    webPreferences.nodeIntegration = false;
    webPreferences.contextIsolation = true;
    webPreferences.sandbox = true;
    webPreferences.webSecurity = true;
  });
  window.webContents.on('did-attach-webview', (_event, guest) => {
    const context = inAgentBrowserBridge?.listContexts().find((candidate) => session.fromPartition(candidate.partition) === guest.session);
    if (context) inAgentBrowserBridge?.attach(context.id, guest);
  });

  if (process.env.ELECTRON_RENDERER_URL) {
    window.loadURL(process.env.ELECTRON_RENDERER_URL);
  } else {
    window.loadFile(desktopBundlePath(app.getAppPath(), 'renderer', 'index.html'));
  }
  installApplicationMenu();
}

function recordRendererExit(reason: string, exitCode: number): void {
  try {
    const directory = join(app.getPath('userData'), 'logs');
    mkdirSync(directory, { recursive: true });
    appendFileSync(join(directory, 'renderer-exits.jsonl'), `${JSON.stringify({
      at: new Date().toISOString(), reason, exitCode
    })}\n`, { mode: 0o600 });
  } catch {
    process.stderr.write(`Beale renderer exited (${reason}, ${exitCode}).\n`);
  }
}

function registerRendererNavigationPolicy(window: BrowserWindow): void {
  const rendererUrl = process.env.ELECTRON_RENDERER_URL;
  const allowedOrigin = rendererUrl ? new URL(rendererUrl).origin : null;
  const allowedFile = desktopBundlePath(app.getAppPath(), 'renderer', 'index.html').replaceAll('\\', '/');
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  window.webContents.on('will-navigate', (event, target) => {
    let allowed = false;
    try {
      const url = new URL(target);
      allowed = allowedOrigin !== null
        ? url.origin === allowedOrigin
        : url.protocol === 'file:' && decodeURIComponent(url.pathname).replaceAll('\\', '/').endsWith(allowedFile);
    } catch {
      allowed = false;
    }
    if (!allowed) event.preventDefault();
  });
}

function reopenMainWindow(): void {
  const existingWindow = mainWindow && !mainWindow.isDestroyed()
    ? mainWindow
    : BrowserWindow.getAllWindows().find((window) => !window.isDestroyed()) ?? null;
  if (restoreAndFocusWindow(existingWindow)) {
    mainWindow = existingWindow;
    return;
  }
  if (app.isReady()) {
    createWindow();
  }
}

function installApplicationMenu(): void {
  if (process.platform !== 'darwin') {
    Menu.setApplicationMenu(null);
    return;
  }
  const window = nativeMenuWindow();
  const zoomPercent = window && !window.isDestroyed()
    ? Math.round(window.webContents.getZoomFactor() * 100)
    : 100;
  Menu.setApplicationMenu(Menu.buildFromTemplate(nativeMacApplicationMenuTemplate(zoomPercent, {
    dispatchRendererAction: sendNativeMenuAction,
    zoomOut: () => adjustNativeZoom(-1),
    zoomIn: () => adjustNativeZoom(1),
    minimizeWindow: () => nativeMenuWindow()?.minimize(),
    maximizeWindow: toggleNativeWindowMaximize,
    closeWindow: () => nativeMenuWindow()?.close()
  })));
}

function nativeMenuWindow(): BrowserWindow | null {
  return BrowserWindow.getFocusedWindow() ?? mainWindow;
}

function sendNativeMenuAction(action: NativeMenuAction): void {
  const window = nativeMenuWindow();
  if (!window || window.isDestroyed()) return;
  window.webContents.send(IPC_CHANNELS.nativeMenuAction, action);
}

function adjustNativeZoom(delta: -1 | 1): void {
  const window = nativeMenuWindow();
  if (!window || window.isDestroyed()) return;
  const nextLevel = Math.max(-4, Math.min(6, window.webContents.getZoomLevel() + delta));
  window.webContents.setZoomLevel(nextLevel);
  installApplicationMenu();
}

function toggleNativeWindowMaximize(): void {
  const window = nativeMenuWindow();
  if (!window || window.isDestroyed()) return;
  if (window.isMaximized()) {
    window.unmaximize();
  } else {
    window.maximize();
  }
}

function createAppIcon(): Electron.NativeImage | null {
  const sourcePath = appIconSourcePath();
  if (!sourcePath) return null;
  const source = nativeImage.createFromPath(sourcePath);
  if (source.isEmpty()) return null;

  const size = source.getSize();
  const cropSize = Math.min(size.width, size.height);
  if (cropSize <= 0) return null;
  const cropped = source.crop({
    x: Math.max(0, Math.floor((size.width - cropSize) / 2)),
    y: Math.max(0, Math.floor((size.height - cropSize) / 2)),
    width: cropSize,
    height: cropSize
  });
  return cropped.resize({ width: 256, height: 256, quality: 'best' });
}

function applyAppIcon(window?: BrowserWindow): void {
  const appIcon = createAppIcon();
  if (!appIcon) return;
  if (process.platform === 'darwin' && app.dock) app.dock.setIcon(appIcon);
  else if (window && !window.isDestroyed()) window.setIcon(appIcon);
}

function appIconSourcePath(): string | null {
  const candidates = [
    join(app.getAppPath(), 'resources/app-icon.png'),
    join(process.cwd(), 'resources/app-icon.png'),
    join(process.resourcesPath, 'app-icon.png'),
    join(process.resourcesPath, 'resources/app-icon.png')
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? null;
}

function windowForEvent(event: IpcMainInvokeEvent): BrowserWindow | null {
  return BrowserWindow.fromWebContents(event.sender);
}

function registerRoundedWindowShape(window: BrowserWindow, enabled: boolean): void {
  if (!enabled) return;

  let pending = false;
  const apply = (): void => {
    if (window.isDestroyed() || pending) return;
    pending = true;
    setImmediate(() => {
      pending = false;
      applyRoundedWindowShape(window);
    });
  };

  window.on('resize', apply);
  window.on('move', apply);
  window.on('maximize', apply);
  window.on('unmaximize', apply);
  window.on('enter-full-screen', apply);
  window.on('leave-full-screen', apply);
  window.webContents.once('did-finish-load', apply);
  apply();
}

function registerWindowStartupShow(window: BrowserWindow, needsExplicitWindowShape: boolean): void {
  let shown = false;
  const show = (): void => {
    if (shown || window.isDestroyed()) return;
    shown = true;
    if (needsExplicitWindowShape) {
      applyRoundedWindowShape(window);
      primeRoundedWindowShapeCompositor(window);
      refreshRoundedWindowShape(window);
    }
    restoreAndFocusWindow(window);
    if (needsExplicitWindowShape) {
      refreshRoundedWindowShape(window);
      setTimeout(() => refreshRoundedWindowShape(window), 120);
      setTimeout(() => refreshRoundedWindowShape(window), 360);
    }
  };

  window.once('ready-to-show', show);
  window.webContents.once('did-finish-load', () => setImmediate(show));
}

function applyRoundedWindowShape(window: BrowserWindow): void {
  if (window.isDestroyed()) return;
  const { width, height } = window.getContentBounds();
  if (width <= 0 || height <= 0) return;
  if (window.isMaximized() || window.isFullScreen()) {
    window.setShape([{ x: 0, y: 0, width, height }]);
    return;
  }
  window.setShape(roundedRectShape(width, height, NATIVE_WINDOW_SHAPE_RADIUS_PX));
}

function refreshRoundedWindowShape(window: BrowserWindow): void {
  if (window.isDestroyed()) return;
  applyRoundedWindowShape(window);
  if (window.isMaximized() || window.isFullScreen()) return;
  const bounds = window.getBounds();
  window.setBounds(bounds, false);
}

function primeRoundedWindowShapeCompositor(window: BrowserWindow): void {
  if (window.isDestroyed() || window.isMaximized() || window.isFullScreen()) return;
  const bounds = window.getBounds();
  window.setBounds({ ...bounds, x: bounds.x + 1 }, false);
  window.setBounds(bounds, false);
}

function windowChromeState(window: BrowserWindow | null): { isMaximized: boolean; isFullScreen: boolean } {
  return {
    isMaximized: window?.isMaximized() ?? false,
    isFullScreen: window?.isFullScreen() ?? false
  };
}

function sendWindowChromeState(window: BrowserWindow): void {
  if (!window.isDestroyed()) {
    window.webContents.send(IPC_CHANNELS.windowChromeStateUpdated, windowChromeState(window));
  }
}

function registerWindowChromeStateEvents(window: BrowserWindow): void {
  const send = (): void => sendWindowChromeState(window);
  window.on('maximize', send);
  window.on('unmaximize', send);
  window.on('enter-full-screen', send);
  window.on('leave-full-screen', send);
  window.webContents.once('did-finish-load', send);
}

function registerRendererDevToolsControls(window: BrowserWindow): void {
  if (!rendererDevToolsAllowed()) return;

  window.webContents.on('before-input-event', (event, input) => {
    if (input.type !== 'keyDown') return;
    const key = input.key.toLowerCase();
    const toggleRequested = input.key === 'F12' || ((input.control || input.meta) && input.shift && key === 'i');
    if (!toggleRequested) return;

    event.preventDefault();
    toggleRendererDevTools(window);
  });

  window.webContents.once('did-finish-load', () => {
    if (rendererDevToolsAutoOpen()) {
      toggleRendererDevTools(window, true);
    }
  });
}

function rendererDevToolsAllowed(): boolean {
  return !app.isPackaged || process.env.BEALE_ENABLE_DEVTOOLS === '1';
}

function rendererDevToolsAutoOpen(): boolean {
  return process.argv.includes('--open-devtools') || process.env.BEALE_OPEN_DEVTOOLS === '1';
}

function toggleRendererDevTools(window: BrowserWindow, openOnly = false): void {
  if (window.isDestroyed()) return;
  if (window.webContents.isDevToolsOpened()) {
    if (!openOnly) {
      window.webContents.closeDevTools();
    }
    return;
  }
  window.webContents.openDevTools({ mode: 'detach' });
}

function timedMainIpc<T>(name: string, detail: Record<string, string | number | boolean>, operation: () => T): T {
  const startedAt = performance.now();
  try {
    return operation();
  } finally {
    const durationMs = performance.now() - startedAt;
    if (mainPerformanceLoggingEnabled()) {
      console.info(`[Beale main perf] ${name} ${roundMetricMs(durationMs)}ms ${formatMainMetricDetail(detail)}`);
    }
    workspaceService?.recordProfilingMainTiming(name, durationMs, detail);
  }
}

async function timedMainIpcAsync<T>(name: string, detail: Record<string, string | number | boolean>, operation: () => Promise<T>): Promise<T> {
  const startedAt = performance.now();
  try {
    return await operation();
  } finally {
    const durationMs = performance.now() - startedAt;
    if (mainPerformanceLoggingEnabled()) {
      console.info(`[Beale main perf] ${name} ${roundMetricMs(durationMs)}ms ${formatMainMetricDetail(detail)}`);
    }
    workspaceService?.recordProfilingMainTiming(name, durationMs, detail);
  }
}

function mainPerformanceLoggingEnabled(): boolean {
  return process.env.BEALE_MAIN_PERF === '1' || process.env.BEALE_DEV_PERFORMANCE === '1';
}

function formatMainMetricDetail(detail: Record<string, string | number | boolean>): string {
  return Object.entries(detail)
    .map(([key, value]) => `${key}=${String(value)}`)
    .join(' ');
}

function roundMetricMs(value: number): number {
  return Math.round(value * 10) / 10;
}

function shortMetricId(id: string): string {
  return id.length <= 12 ? id : `${id.slice(0, 6)}...${id.slice(-4)}`;
}

function broadcastSnapshot(change: WorkspaceChange = { workspaceRegistryChanged: true }): void {
  const snapshotChanged = change.snapshotChanged !== false;
  if (!snapshotChanged && !change.workspaceRegistryChanged) return;
  timedMainIpc('broadcastSnapshot.total', { registry: change.workspaceRegistryChanged, snapshot: snapshotChanged }, () => {
    const snapshot = snapshotChanged
      ? timedMainIpc('broadcastSnapshot.getSnapshot', {}, () => workspaceService.getSnapshot())
      : null;
    const workspaceRegistry = change.workspaceRegistryChanged
      ? timedMainIpc('broadcastSnapshot.getWorkspaceRegistry', snapshotBroadcastMetricDetail(snapshot), () => workspaceService.getCachedWorkspaceRegistryState())
      : null;
    const windows = BrowserWindow.getAllWindows();
    timedMainIpc(
      'broadcastSnapshot.sendAll',
      {
        ...snapshotBroadcastMetricDetail(snapshot),
        ...(workspaceRegistry ? workspaceRegistryBroadcastMetricDetail(workspaceRegistry) : { registryWorkspaces: 0, registrySessions: 0 }),
        registry: Boolean(workspaceRegistry),
        windows: windows.length
      },
      () => {
        for (const window of windows) {
          if (snapshotChanged) {
            window.webContents.send(IPC_CHANNELS.snapshotUpdated, snapshot);
          }
          if (workspaceRegistry) {
            window.webContents.send(IPC_CHANNELS.workspaceRegistryUpdated, workspaceRegistry);
          }
        }
      }
    );
  });
}

function broadcastIosDeviceCaptureState(state: import('@shared/types').IosDeviceCaptureState): void {
  for (const window of BrowserWindow.getAllWindows()) {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.iosDeviceCaptureUpdated, state);
  }
}

function broadcastIosDeviceCaptureFrame(frame: import('@shared/types').IosDeviceCaptureFrame): void {
  for (const window of BrowserWindow.getAllWindows()) {
    if (!window.isDestroyed()) window.webContents.send(IPC_CHANNELS.iosDeviceCaptureFrame, frame);
  }
}

function snapshotBroadcastMetricDetail(snapshot: WorkspaceSnapshot | null): Record<string, string | number | boolean> {
  return {
    active: Boolean(snapshot),
    runs: snapshot?.runs.length ?? 0,
    notifications: snapshot?.notifications.length ?? 0,
    workspace: Boolean(snapshot?.workspace)
  };
}

function workspaceRegistryBroadcastMetricDetail(workspaceRegistry: WorkspaceRegistryState): Record<string, string | number | boolean> {
  return {
    registryWorkspaces: workspaceRegistry.workspaces.length,
    registrySessions: workspaceRegistry.researchSessions.length
  };
}

function registerIpc(): void {
  ipcMain.handle(IPC_CHANNELS.getDesktopUpdateState, () => desktopUpdateState());
  ipcMain.handle(IPC_CHANNELS.downloadDesktopUpdate, () => desktopUpdater?.download() ?? desktopUpdateState());
  ipcMain.handle(IPC_CHANNELS.installDesktopUpdate, () => desktopUpdater?.install() ?? desktopUpdateState());
  ipcMain.handle(IPC_CHANNELS.listBrowserContexts, () => inAgentBrowserBridge?.listContexts() ?? []);
  ipcMain.handle(IPC_CHANNELS.fleetBrowserMachine, (_event, runId: string) => workspaceService.fleetBrowserMachineId(runId));
  ipcMain.handle(IPC_CHANNELS.connectFleetBrowser, (_event, runId: string, remoteServerId?: string) => {
    if (remoteServerId) { fleetBrowserViewer.connectRemote(runId, remoteServerId); return; }
    const machineId = workspaceService.fleetBrowserMachineId(runId);
    if (!machineId) throw new Error('This session does not use a streamed Fleet browser.');
    fleetBrowserViewer.connect(runId, machineId);
  });
  ipcMain.handle(IPC_CHANNELS.disconnectFleetBrowser, (_event, runId: string) => fleetBrowserViewer.disconnect(runId));
  ipcMain.handle(IPC_CHANNELS.fleetBrowserInput, (_event, runId: string, input: FleetBrowserInput) => fleetBrowserViewer.input(runId, input));
  ipcMain.handle(IPC_CHANNELS.createBrowserContext, (_event, label: string) => {
    if (!inAgentBrowserBridge) throw new Error('The embedded browser is unavailable.');
    return inAgentBrowserBridge.createContext(label);
  });
  ipcMain.handle(IPC_CHANNELS.renameBrowserContext, (_event, id: string, label: string) => {
    if (!inAgentBrowserBridge) throw new Error('The embedded browser is unavailable.');
    return inAgentBrowserBridge.renameContext(id, label);
  });
  ipcMain.handle(IPC_CHANNELS.removeBrowserContext, async (_event, id: string) => {
    if (!inAgentBrowserBridge) throw new Error('The embedded browser is unavailable.');
    await inAgentBrowserBridge.removeContext(id);
  });
  ipcMain.handle(IPC_CHANNELS.selectWorkspace, async (_event, mode: WorkspacePickerMode) => {
    const result = await dialog.showOpenDialog({
      title: mode === 'create' ? 'Create Beale workspace' : 'Import Beale workspace',
      properties: mode === 'create' ? ['openDirectory', 'createDirectory'] : ['openDirectory']
    });
    return {
      canceled: result.canceled,
      path: result.filePaths[0] ?? null
    };
  });

  ipcMain.handle(IPC_CHANNELS.selectWorkspaceDirectory, async () => {
    const result = await dialog.showOpenDialog({
      title: 'Add Beale workspace',
      properties: ['openDirectory', 'createDirectory']
    });
    const path = result.filePaths[0] ?? null;
    return result.canceled || !path
      ? {
          canceled: true,
          path: null,
          knownWorkspace: null,
          requiresOnboarding: false,
          defaults: null
        }
      : workspaceService.inspectWorkspaceDirectory(path);
  });
  ipcMain.handle(IPC_CHANNELS.getWorkspaceRegistry, () =>
    timedMainIpcAsync('getWorkspaceRegistry', {}, () => workspaceService.getWorkspaceRegistryStateForClient())
  );
  ipcMain.handle(IPC_CHANNELS.listArchivedQuickChats, () => workspaceService.listArchivedQuickChats());
  ipcMain.handle(IPC_CHANNELS.archiveResearchSession, (_event, sessionId: string) =>
    workspaceService.archiveResearchSession(sessionId)
  );
  ipcMain.handle(IPC_CHANNELS.restoreResearchSession, (_event, sessionId: string) =>
    workspaceService.restoreResearchSession(sessionId)
  );
  ipcMain.handle(IPC_CHANNELS.markResearchSessionViewed, (_event, sessionId: string) =>
    workspaceService.markResearchSessionViewed(sessionId)
  );
  ipcMain.handle(IPC_CHANNELS.getDeveloperSettings, () => workspaceService.getDeveloperSettings());
  ipcMain.handle(IPC_CHANNELS.setDeveloperModeEnabled, (_event, enabled: boolean) => workspaceService.setDeveloperModeEnabled(enabled));
  ipcMain.handle(IPC_CHANNELS.getDebuggingSettings, () => workspaceService.getDebuggingSettings());
  ipcMain.handle(IPC_CHANNELS.setTracesEnabled, (_event, enabled: boolean) => workspaceService.setTracesEnabled(enabled));
  ipcMain.handle(IPC_CHANNELS.getAppServerRemoteAccessSettings, (_event, detect: boolean) => (
    detect ? detectAppServerMagicDnsName() : readAppServerRemoteAccessSettings()
  ));
  ipcMain.handle(IPC_CHANNELS.setAppServerRemoteAccessSettings, async (
    _event,
    update: AppServerRemoteAccessUpdate
  ) => {
    const settings = await updateAppServerRemoteAccess(update);
    await restartBealeAppServer();
    return settings;
  });
  ipcMain.handle(IPC_CHANNELS.getProviderSettings, () => workspaceService.getProviderSettings());
  ipcMain.handle(IPC_CHANNELS.setDefaultProviderId, (_event, providerId: ResearchModelProviderId | null) => workspaceService.setDefaultProviderId(providerId));
  ipcMain.handle(IPC_CHANNELS.saveCustomProvider, (_event, input: { id?: `custom-${string}`; name: string; baseUrl: string; contextWindow?: number; maxTokens?: number; apiKey?: string; modelAllowlist?: string[]; modelOffReasoningEfforts?: Record<string, string>; reasoning?: boolean }) =>
    workspaceService.saveCustomProvider(input));
  ipcMain.handle(IPC_CHANNELS.removeCustomProvider, (_event, providerId: `custom-${string}`) =>
    workspaceService.removeCustomProvider(providerId));
  ipcMain.handle(IPC_CHANNELS.setProviderModelDefaults, (_event, providerId: ResearchModelProviderId, defaults: ProviderModelDefaults) =>
    workspaceService.setProviderModelDefaults(providerId, defaults)
  );
  ipcMain.handle(IPC_CHANNELS.setProviderContextSize, (
    _event,
    providerId: ResearchModelProviderId,
    contextSize: ProviderContextSize
  ) => workspaceService.setProviderContextSize(providerId, contextSize));
  ipcMain.handle(IPC_CHANNELS.setProviderOptionalModelEnabled, (
    _event,
    providerId: ResearchModelProviderId,
    modelId: string,
    enabled: boolean
  ) => workspaceService.setProviderOptionalModelEnabled(providerId, modelId, enabled));
  ipcMain.handle(IPC_CHANNELS.setProviderCyberPolicyRiskAcknowledged, (
    _event,
    providerId: ResearchModelProviderId,
    acknowledged: boolean
  ) =>
    workspaceService.setProviderCyberPolicyRiskAcknowledged(providerId, acknowledged)
  );
  ipcMain.handle(IPC_CHANNELS.setProviderPreferredAuthenticationMethod, (
    _event,
    providerId: ResearchModelProviderId,
    method: ProviderAuthenticationMethod
  ) => workspaceService.setProviderPreferredAuthenticationMethod(providerId, method));
  const ticketingUnavailable = (): never => { throw new Error('Ticketing is temporarily unavailable.'); };
  ipcMain.handle(IPC_CHANNELS.getTicketingSettings, ticketingUnavailable);
  ipcMain.handle(IPC_CHANNELS.setTicketingProvider, ticketingUnavailable);
  ipcMain.handle(IPC_CHANNELS.setTicketingHumanInTheLoop, ticketingUnavailable);
  ipcMain.handle(IPC_CHANNELS.configureTicketingCredential, ticketingUnavailable);
  ipcMain.handle(IPC_CHANNELS.removeTicketingCredential, ticketingUnavailable);
  ipcMain.handle(IPC_CHANNELS.listTicketingTargets, ticketingUnavailable);
  ipcMain.handle(IPC_CHANNELS.setTicketingTarget, ticketingUnavailable);
  ipcMain.handle(IPC_CHANNELS.submitReportTicket, ticketingUnavailable);
  ipcMain.handle(IPC_CHANNELS.getResearchProfiles, () => workspaceService.getResearchProfiles());
  ipcMain.handle(IPC_CHANNELS.getPromptTemplateSettings, (_event, profileId: ResearchProfileId) => workspaceService.getPromptTemplateSettings(profileId));
  ipcMain.handle(IPC_CHANNELS.getFeaturePromptSettings, () => workspaceService.getFeaturePromptSettings());
  ipcMain.handle(IPC_CHANNELS.setFeaturePromptSettings, (_event, featureId: string, description: string, promptExtension: string) =>
    workspaceService.setFeaturePromptSettings(featureId, description, promptExtension));
  ipcMain.handle(IPC_CHANNELS.resetFeaturePromptSettings, (_event, featureId: string) => workspaceService.resetFeaturePromptSettings(featureId));
  ipcMain.handle(IPC_CHANNELS.setPromptTemplate, (_event, profileId: ResearchProfileId, template: string) => workspaceService.setPromptTemplate(profileId, template));
  ipcMain.handle(IPC_CHANNELS.resetPromptTemplate, (_event, profileId: ResearchProfileId) => workspaceService.resetPromptTemplate(profileId));
  ipcMain.handle(IPC_CHANNELS.previewPromptTemplate, (_event, profileId: ResearchProfileId, template: string, agentPath?: string) => workspaceService.previewPromptTemplate(profileId, template, agentPath));
  ipcMain.handle(IPC_CHANNELS.getAgentPlugins, () => workspaceService.getAgentPlugins());
  ipcMain.handle(IPC_CHANNELS.getFleetState, (_event, workspaceId?: string) => workspaceService.getFleetState(workspaceId));
  ipcMain.handle(IPC_CHANNELS.getFleetAppServerStatuses, async () => {
    const local = await inspectLocalAppServerStatus();
    if (local.state !== 'online') return [local];
    try { return [local, ...await workspaceService.getFleetAppServerStatuses()]; }
    catch { return [local]; }
  });
  ipcMain.handle(IPC_CHANNELS.getFleetPairingQrCode, (_event, serverId: string) => {
    if (typeof serverId !== 'string' || !serverId.trim()) throw new Error('Select an app server to show its QR code.');
    return workspaceService.getFleetPairingQrCode(serverId);
  });
  ipcMain.handle(IPC_CHANNELS.getBrowserHomePage, () => workspaceService.getBrowserHomePage());
  ipcMain.handle(IPC_CHANNELS.restartLocalAppServer, async () => { await restartBealeAppServerViaSupervisor(); });
  ipcMain.handle(IPC_CHANNELS.testLocalAppServerConnection, () => testLocalAppServerConnection());
  ipcMain.handle(IPC_CHANNELS.restartFleetAppServer, (_event, serverId: string) => workspaceService.restartFleetAppServer(serverId));
  ipcMain.handle(IPC_CHANNELS.restartFleetGuestAppServer, (_event, machineId: string) => workspaceService.restartFleetGuestAppServer(machineId));
  ipcMain.handle(IPC_CHANNELS.getFleetRemoteMachines, () => workspaceService.getFleetRemoteMachines());
  ipcMain.handle(IPC_CHANNELS.getFleetRemoteCatalog, (_event, serverId: string) => workspaceService.getFleetRemoteCatalog(serverId));
  ipcMain.handle(IPC_CHANNELS.getFleetRemoteWorkspaceDetails, (_event, serverId: string, workspaceId: string) => workspaceService.getFleetRemoteWorkspaceDetails(serverId, workspaceId));
  ipcMain.handle(IPC_CHANNELS.getWorkspaceSurfaceMap, (_event, workspaceId: string, sessionId?: string) => workspaceService.getWorkspaceSurfaceMap(workspaceId, sessionId));
  ipcMain.handle(IPC_CHANNELS.getResearchTrack, (_event, workspaceId: string, serverId?: string) => workspaceService.getResearchTrack(workspaceId, serverId));
  ipcMain.handle(IPC_CHANNELS.controlResearchTrack, (_event, workspaceId: string, action: 'start' | 'pause' | 'resume' | 'stop', serverId?: string,
    configuration?: import('@beale/app-server-runtime/protocol').BealeWorkspaceResearchTrackRequest['configuration']) => workspaceService.controlResearchTrack(workspaceId, action, serverId, configuration));
  ipcMain.handle(IPC_CHANNELS.getFleetRemoteWorkspaceDocument, (_event, serverId: string, workspaceId: string, kind: 'runbook' | 'report', resourceId: string) => workspaceService.getFleetRemoteWorkspaceDocument(serverId, workspaceId, kind, resourceId));
  ipcMain.handle(IPC_CHANNELS.getFleetRemoteQuickChats, (_event, serverId: string) => workspaceService.getFleetRemoteQuickChats(serverId));
  ipcMain.handle(IPC_CHANNELS.startFleetRemoteQuickChat, (_event, serverId: string, input: import('@beale/app-server-runtime/protocol').BealeAppServerQuickChatStartInput) => workspaceService.startFleetRemoteQuickChat(serverId, input));
  ipcMain.handle(IPC_CHANNELS.continueFleetRemoteQuickChat, (_event, serverId: string, workspaceId: string, sessionId: string, instruction: string) => workspaceService.continueFleetRemoteQuickChat(serverId, workspaceId, sessionId, instruction));
  ipcMain.handle(IPC_CHANNELS.getFleetRemoteSession, (_event, serverId: string, workspaceId: string, sessionId: string, beforeEventId?: string) => workspaceService.getFleetRemoteSession(serverId, workspaceId, sessionId, beforeEventId));
  ipcMain.handle(IPC_CHANNELS.startFleetRemoteSession, (_event, serverId: string, workspaceId: string, research: import('@beale/app-server-runtime/protocol').FleetRemoteResearchInput) => workspaceService.startFleetRemoteSession(serverId, workspaceId, research));
  ipcMain.handle(IPC_CHANNELS.expandFleetRemoteResearchPrompt, (_event, serverId: string, workspaceId: string, expansion: import('@beale/app-server-runtime/protocol').FleetRemotePromptExpansionInput) => workspaceService.expandFleetRemoteResearchPrompt(serverId, workspaceId, expansion));
  ipcMain.handle(IPC_CHANNELS.getFleetRemoteResearchGoalSuggestions, (_event, serverId: string, workspaceId: string, phase: string, refresh: boolean, incremental = false) => workspaceService.getFleetRemoteResearchGoalSuggestions(serverId, workspaceId, phase, refresh, incremental));
  ipcMain.handle(IPC_CHANNELS.controlFleetRemoteSession, (_event, serverId: string, sessionId: string, type: 'pause' | 'resume' | 'stop' | 'steer', instruction?: string) => workspaceService.controlFleetRemoteSession(serverId, sessionId, type, instruction));
  ipcMain.handle(IPC_CHANNELS.configureFleet, (_event, input: Record<string, unknown>) => workspaceService.configureFleet(input));
  ipcMain.handle(IPC_CHANNELS.testFleetVmConnection, (_event, input: import('@beale/app-server-runtime/protocol').FleetSshTestInput) => workspaceService.testFleetVmConnection(input));
  ipcMain.handle(IPC_CHANNELS.testFleetAppServer, (_event, input: { serverId?: string; url: string; operatorToken: string }) => workspaceService.testFleetAppServer(input));
  ipcMain.handle(IPC_CHANNELS.cloneFleetVm, (_event, baseId: string, name: string) => workspaceService.cloneFleetVm(baseId, name));
  ipcMain.handle(IPC_CHANNELS.startFleetVm, (_event, machineId: string) => workspaceService.startFleetVm(machineId));
  ipcMain.handle(IPC_CHANNELS.stopFleetVm, (_event, machineId: string) => workspaceService.stopFleetVm(machineId));
  ipcMain.handle(IPC_CHANNELS.addAgentPluginFromFilesystem, async () => {
    const result = await dialog.showOpenDialog({
      title: 'Add Agent Plugin',
      properties: ['openDirectory']
    });
    const path = result.filePaths[0] ?? null;
    return result.canceled || !path
      ? workspaceService.getAgentPlugins()
      : workspaceService.addAgentPluginFromFilesystem(path);
  });
  ipcMain.handle(IPC_CHANNELS.addAgentPluginFromRepository, (_event, repositoryUrl: string) =>
    workspaceService.addAgentPluginFromRepository(repositoryUrl)
  );
  ipcMain.handle(IPC_CHANNELS.setAgentPluginEnabled, (_event, pluginId: string, enabled: boolean, workspaceId?: string) =>
    workspaceService.setAgentPluginEnabled(pluginId, enabled, workspaceId)
  );
  ipcMain.handle(IPC_CHANNELS.removeAgentPlugin, (_event, pluginId: string) => workspaceService.removeAgentPlugin(pluginId));
  ipcMain.handle(IPC_CHANNELS.getMemorySettings, () => workspaceService.getMemorySettings());
  ipcMain.handle(IPC_CHANNELS.setMemoryTypeDescriptions, (_event, descriptions: MemoryTypeDescriptions) => workspaceService.setMemoryTypeDescriptions(descriptions));
  ipcMain.handle(IPC_CHANNELS.getShellOptions, () => workspaceService.getShellOptions());
  ipcMain.handle(IPC_CHANNELS.setShellOptions, (_event, options: ShellOptions) => workspaceService.setShellOptions(options));
  ipcMain.handle(IPC_CHANNELS.lookupHackerOneScope, (_event, identifier: string) => workspaceService.lookupHackerOneScope(identifier));
  ipcMain.handle(IPC_CHANNELS.lookupBugcrowdScope, (_event, identifier: string) => workspaceService.lookupBugcrowdScope(identifier));
  ipcMain.handle(IPC_CHANNELS.refreshResearchKit, (_event, input: ResearchKitRefreshInput) => workspaceService.refreshResearchKit(input));
  ipcMain.handle(IPC_CHANNELS.listGitHubOrganizationRepositories, (_event, organization: string) =>
    workspaceService.listGitHubOrganizationRepositories(organization)
  );
  ipcMain.handle(IPC_CHANNELS.createScopedWorkspace, (event, input: WorkspaceOnboardingInput) =>
    workspaceService.createScopedWorkspace(input, (update) => event.sender.send(IPC_CHANNELS.workspaceOnboardingUpdated, update))
  );
  ipcMain.handle(IPC_CHANNELS.updateWorkspaceDirectories, (_event, directories: string[]) =>
    workspaceService.updateWorkspaceDirectories(directories)
  );
  ipcMain.handle(IPC_CHANNELS.updateWorkspaceMemoryBackend, (_event, memoryBackend: WorkspaceMemoryBackendId) =>
    workspaceService.updateWorkspaceMemoryBackend(memoryBackend)
  );
  ipcMain.handle(IPC_CHANNELS.updateWorkspaceResearchSubject, (_event, researchSubjectName: string) =>
    workspaceService.updateWorkspaceResearchSubject(researchSubjectName)
  );
  ipcMain.handle(IPC_CHANNELS.cloneWorkspaceRepository, (_event, assetId: string, cloneMode: RepositoryCloneMode) =>
    workspaceService.cloneWorkspaceRepository(assetId, cloneMode)
  );
  ipcMain.handle(IPC_CHANNELS.listResourcePriorArt, (_event, workspaceId: string, assetIds: string[], before?: number) => workspaceService.listResourcePriorArt(workspaceId, assetIds, before));
  ipcMain.handle(IPC_CHANNELS.getResourcePriorArt, (_event, workspaceId: string, assetIds: string[], id: string, offset?: number) => workspaceService.getResourcePriorArt(workspaceId, assetIds, id, offset));
  ipcMain.handle(IPC_CHANNELS.skipWorkspaceOnboardingRepository, (_event, input: WorkspaceOnboardingSkipInput) => workspaceService.skipWorkspaceOnboardingRepository(input));
  ipcMain.handle(IPC_CHANNELS.openRegisteredWorkspace, (_event, registryWorkspaceId: string) =>
    timedMainIpc('openRegisteredWorkspace', { workspace: shortMetricId(registryWorkspaceId) }, () => workspaceService.openRegisteredWorkspace(registryWorkspaceId))
  );
  ipcMain.handle(IPC_CHANNELS.removeRegisteredWorkspace, (_event, registryWorkspaceId: string) => workspaceService.removeRegisteredWorkspace(registryWorkspaceId));
  ipcMain.handle(IPC_CHANNELS.openWorkspace, (_event, path: string) => workspaceService.openWorkspace(path));
  ipcMain.handle(IPC_CHANNELS.createWorkspace, (_event, path: string) => workspaceService.createWorkspace(path));
  ipcMain.handle(IPC_CHANNELS.restoreLastWorkspace, () =>
    timedMainIpc('restoreLastWorkspace', {}, () => workspaceService.openLastWorkspaceIfAvailable())
  );
  ipcMain.handle(IPC_CHANNELS.getSnapshot, () => timedMainIpc('getSnapshot', {}, () => workspaceService.getSnapshot()));
  ipcMain.handle(IPC_CHANNELS.getHostEnvironment, () => getHostEnvironment());
  ipcMain.handle(IPC_CHANNELS.getWorkspaceEditors, () => getWorkspaceEditorCatalogForHost({}, async (path) => {
    const icon = await app.getFileIcon(path, { size: 'normal' });
    return icon.isEmpty() ? null : icon.toDataURL();
  }));
  ipcMain.handle(IPC_CHANNELS.openWorkspaceInEditor, async (_event, editorId: WorkspaceEditorId) => {
    const workspacePath = workspaceService.getSnapshot()?.workspace.workspacePath;
    if (!workspacePath) throw new Error('No Beale workspace is open.');
    await openWorkspaceInEditor(editorId, workspacePath);
  });
  ipcMain.handle(IPC_CHANNELS.getIosDeviceCaptureState, () => iosDeviceCaptureService.getState());
  ipcMain.handle(IPC_CHANNELS.startIosDeviceCapture, () => iosDeviceCaptureService.start());
  ipcMain.handle(IPC_CHANNELS.stopIosDeviceCapture, () => iosDeviceCaptureService.stop());
  ipcMain.handle(IPC_CHANNELS.getOpenAiStatus, () => workspaceService.getOpenAiStatus());
  ipcMain.handle(IPC_CHANNELS.startOpenAiOAuth, () => workspaceService.startOpenAiOAuth());
  ipcMain.handle(IPC_CHANNELS.forgetProviderSubscription, (_event, providerId: ResearchModelProviderId) =>
    workspaceService.forgetProviderSubscription(providerId));
  ipcMain.handle(IPC_CHANNELS.removeProvider, (_event, providerId: ResearchModelProviderId) =>
    workspaceService.removeProvider(providerId));
  ipcMain.handle(IPC_CHANNELS.configureProviderApiKey, (_event, providerId: ResearchModelProviderId, apiKey: string) =>
    workspaceService.configureProviderApiKey(providerId, apiKey));
  ipcMain.handle(IPC_CHANNELS.removeProviderApiKey, (_event, providerId: ResearchModelProviderId) =>
    workspaceService.removeProviderApiKey(providerId));
  ipcMain.handle(IPC_CHANNELS.getProviderCredentialAccessRequest, (_event, providerIds: ResearchModelProviderId[]) =>
    workspaceService.getProviderCredentialAccessRequest(providerIds));
  ipcMain.handle(IPC_CHANNELS.unlockProviderApiKeys, (_event, providerIds: ResearchModelProviderId[]) =>
    workspaceService.unlockProviderApiKeys(providerIds));
  ipcMain.handle(IPC_CHANNELS.refreshOpenAiStatus, () => workspaceService.refreshOpenAiStatus());
  ipcMain.handle(IPC_CHANNELS.getResearchProviderStatuses, () => workspaceService.getResearchProviderStatuses());
  ipcMain.handle(IPC_CHANNELS.getProviderUsageLimits, (_event, providerId: ResearchModelProviderId, refresh?: boolean, serverId?: string) => {
    if (providerId.startsWith('custom-')) throw new Error('Subscription usage limits are unavailable for custom providers.');
    return workspaceService.getProviderUsageLimits(providerId as Parameters<typeof workspaceService.getProviderUsageLimits>[0], refresh, serverId);
  });
  ipcMain.handle(IPC_CHANNELS.getResearchProviderModelCatalog, () => workspaceService.getResearchProviderModelCatalog());
  ipcMain.handle(IPC_CHANNELS.startResearchProviderOAuth, async (_event, providerId: ResearchProviderId) => {
    const result = await workspaceService.startResearchProviderOAuth(providerId);
    if (result.verificationUri) {
      const url = new URL(result.verificationUri);
      if (url.protocol !== 'https:') throw new Error('Provider authentication returned an untrusted URL.');
      await shell.openExternal(url.href);
    }
    return result;
  });
  ipcMain.handle(IPC_CHANNELS.getProfilingState, () => workspaceService.getProfilingState());
  ipcMain.handle(IPC_CHANNELS.setProfilingEnabled, (_event, enabled: boolean) => workspaceService.setProfilingEnabled(enabled));
  ipcMain.handle(IPC_CHANNELS.recordProfilingReport, (_event, report: ProfilingReport) => workspaceService.recordProfilingReport(report));
  ipcMain.handle(IPC_CHANNELS.openAppServerMemoryDirectory, (_event, name: AppServerMemoryDirectorySummary['name']) =>
    timedMainIpcAsync('openAppServerMemoryDirectory', { directory: String(name) }, async () => {
      const path = workspaceService.resolveAppServerMemoryDirectoryPath(name);
      const error = await shell.openPath(path);
      if (error) throw new Error(error);
    })
  );
  ipcMain.handle(IPC_CHANNELS.getAppServerRunbook, (_event, runbookId: string) =>
    timedMainIpcAsync('getAppServerRunbook', { runbook: shortMetricId(runbookId) }, () =>
      workspaceService.getAppServerRunbook(runbookId)
    )
  );
  ipcMain.handle(IPC_CHANNELS.listAutomations, () =>
    timedMainIpcAsync('listAutomations', {}, () => workspaceService.listAutomations())
  );
    ipcMain.handle(IPC_CHANNELS.updateAutomation, (_event, input: AutomationUpdateInput) =>
      timedMainIpc('updateAutomation', { run: shortMetricId(input.runId) }, () => workspaceService.updateAutomation(input))
    );
  ipcMain.handle(IPC_CHANNELS.listReportingReports, () =>
    timedMainIpcAsync('listReportingReports', {}, () => workspaceService.listReportingReports())
  );
  ipcMain.handle(IPC_CHANNELS.getAppServerReport, (_event, locator: AppServerReportLocator) =>
    timedMainIpc('getAppServerReport', { report: shortMetricId(locator.reportId) }, () =>
      workspaceService.getAppServerReport(locator)
    )
  );
  ipcMain.handle(IPC_CHANNELS.updateReportContent, (_event, input: ReportContentUpdateInput) =>
    timedMainIpcAsync('updateReportContent', { report: shortMetricId(input.reportId) }, () =>
      workspaceService.updateReportContent(input)
    )
  );
  ipcMain.handle(IPC_CHANNELS.updateReportTriageStatus, (_event, input: ReportTriageStatusUpdateInput) =>
    timedMainIpcAsync('updateReportTriageStatus', { report: shortMetricId(input.reportId) }, () =>
      workspaceService.updateReportTriageStatus(input)
    )
  );
  ipcMain.handle(IPC_CHANNELS.getReportSubmissionPacketPath, (_event, locator: AppServerReportLocator) =>
    timedMainIpcAsync('getReportSubmissionPacketPath', { report: shortMetricId(locator.reportId) }, () =>
      workspaceService.resolveReportSubmissionPacketPath(locator)
    )
  );
  ipcMain.handle(IPC_CHANNELS.openReportSubmissionPacket, async (_event, locator: AppServerReportLocator) =>
    timedMainIpcAsync('openReportSubmissionPacket', { report: shortMetricId(locator.reportId) }, async () => {
      const path = await workspaceService.resolveReportSubmissionPacketPath(locator);
      shell.showItemInFolder(path);
    })
  );
  ipcMain.handle(IPC_CHANNELS.chooseReportSubmissionPacket, async (_event, locator: AppServerReportLocator) => {
    const result = await dialog.showOpenDialog({
      title: 'Choose report submission packet',
      properties: ['openFile'],
      filters: [{ name: 'ZIP archives', extensions: ['zip'] }]
    });
    const path = result.filePaths[0] ?? null;
    return result.canceled || !path ? null : workspaceService.replaceReportSubmissionPacket(locator, path);
  });
  ipcMain.handle(IPC_CHANNELS.chooseReportRecording, async (_event, locator: AppServerReportLocator) => {
    const result = await dialog.showOpenDialog({
      title: 'Choose report recording',
      properties: ['openFile'],
      filters: [{
        name: 'Media files',
        extensions: ['aac', 'flac', 'm4a', 'm4v', 'mkv', 'mov', 'mp3', 'mp4', 'ogg', 'opus', 'wav', 'webm']
      }]
    });
    const path = result.filePaths[0] ?? null;
    return result.canceled || !path ? null : workspaceService.replaceReportRecording(locator, path);
  });
  ipcMain.handle(IPC_CHANNELS.openExternalUrl, async (_event, value: string) => {
    const url = new URL(value);
    if (url.protocol !== 'https:' || (url.hostname !== 'github.com' && url.hostname !== 'linear.app')) {
      throw new Error('Only GitHub and Linear HTTPS ticket links can be opened externally.');
    }
    await shell.openExternal(url.toString());
  });
  ipcMain.handle(IPC_CHANNELS.openContentLink, async (_event, value: string) => {
    const target = resolveContentLink(value, workspaceService.getSnapshot()?.workspace.workspacePath ?? null);
    if (target.kind === 'web') {
      await shell.openExternal(target.url);
    } else if (target.directory) {
      const error = await shell.openPath(target.path);
      if (error) throw new Error(error);
    } else {
      shell.showItemInFolder(target.path);
    }
  });
  ipcMain.handle(IPC_CHANNELS.startReportSession, (_event, input: ReportSessionStartInput) =>
    timedMainIpc('startReportSession', { report: shortMetricId(input.reportId) }, () =>
      workspaceService.startReportSession(input)
    )
  );
  ipcMain.handle(IPC_CHANNELS.markHistoryDuplicate, (_event, input: MarkHistoryDuplicateInput) =>
    timedMainIpcAsync('markHistoryDuplicate', { type: input.type, record: shortMetricId(input.id) }, () =>
      workspaceService.markHistoryDuplicate(input)
    )
  );
  ipcMain.handle(IPC_CHANNELS.undoHistoryDuplicate, (_event, input: UndoHistoryDuplicateInput) =>
    timedMainIpcAsync('undoHistoryDuplicate', { type: input.type, record: shortMetricId(input.id) }, () =>
      workspaceService.undoHistoryDuplicate(input)
    )
  );
  ipcMain.handle(IPC_CHANNELS.transitionClaimOnBoard, (_event, input: ClaimBoardTransitionRequest) =>
    timedMainIpcAsync('transitionClaimOnBoard', { record: shortMetricId(input?.claimId) }, () =>
      workspaceService.transitionClaimOnBoard(input)
    )
  );
  ipcMain.handle(IPC_CHANNELS.getAppServerToolingSummary, () =>
    timedMainIpc('getAppServerToolingSummary', {}, () => workspaceService.getAppServerToolingSummary())
  );
  ipcMain.handle(IPC_CHANNELS.updateAppServerToolingConfig, (_event, update: AppServerToolingConfigUpdate) =>
    timedMainIpc('updateAppServerToolingConfig', { type: update.type }, () => workspaceService.updateAppServerToolingConfig(update))
  );
  ipcMain.handle(IPC_CHANNELS.generateResearchGoalSuggestions, async (_event, input: ResearchGoalSuggestionInput) => {
    const workspaceId = workspaceService.getSnapshot()?.workspace.workspaceId;
    if (!workspaceId) throw new Error('No Beale workspace is open');
    const requestId = input.requestId?.trim() || null;
    const controller = new AbortController();
    if (requestId) {
      researchGoalSuggestionControllers.get(requestId)?.abort();
      researchGoalSuggestionControllers.set(requestId, controller);
    }
    try {
      return await timedMainIpcAsync('generateResearchGoalSuggestions', {}, async () =>
        decodeGeneratedResearchGoalSuggestions(await invokeAppServerOperation<unknown>({
          operation: 'suggestion.generate',
          input: { ...input, workspaceId },
          signal: controller.signal
        }), input.phase)
      );
    } finally {
      if (requestId && researchGoalSuggestionControllers.get(requestId) === controller) {
        researchGoalSuggestionControllers.delete(requestId);
      }
    }
  });
  ipcMain.handle(IPC_CHANNELS.generateSteeringSuggestion, (_event, runId: string) => {
    const workspaceId = workspaceService.getSnapshot()?.workspace.workspaceId;
    if (!workspaceId) throw new Error('No Beale workspace is open');
    return timedMainIpcAsync('generateSteeringSuggestion', {}, () =>
      invokeAppServerOperation<SteeringSuggestionResult>({
        operation: 'suggestion.steering',
        input: { workspaceId, sessionId: runId }
      })
    );
  });
  ipcMain.handle(IPC_CHANNELS.selectResearchGoalSuggestion, async (_event, input: ResearchGoalSuggestionSelectionInput) => {
    await timedMainIpcAsync('selectResearchGoalSuggestion', {}, () =>
      invokeAppServerOperation<{ selected: true }>({ operation: 'suggestion.select', input })
    );
  });
  ipcMain.handle(IPC_CHANNELS.generateResearchPrompt, (event, input?: ResearchPromptGenerationInput) =>
    timedMainIpcAsync('generateResearchPrompt', { hasInput: Boolean(input) }, () =>
      workspaceService.generateResearchPrompt(input, (update) => event.sender.send(IPC_CHANNELS.researchPromptGenerationUpdated, update))
    )
  );
  ipcMain.handle(IPC_CHANNELS.cancelResearchPromptGeneration, (_event, requestId: string) => {
    researchGoalSuggestionControllers.get(requestId)?.abort();
    researchGoalSuggestionControllers.delete(requestId);
    workspaceService.cancelResearchPromptGeneration(requestId);
  });
  ipcMain.handle(IPC_CHANNELS.saveScope, (_event, scope: WorkspaceScopeDraft) => workspaceService.saveScope(scope));
  ipcMain.handle(IPC_CHANNELS.addWorkspaceRule, (_event, text: string) => workspaceService.addWorkspaceRule(text));
  ipcMain.handle(IPC_CHANNELS.startRun, (_event, input: StartRunInput) =>
    timedMainIpcAsync('startRun', { engine: input.runEngine, mode: input.mode }, () =>
      workspaceService.startRunWithSourcePreparation(input)
    )
  );
  ipcMain.handle(IPC_CHANNELS.startQuickChat, (_event, input: QuickChatStartInput) =>
    timedMainIpcAsync('startQuickChat', {}, () => workspaceService.startQuickChat(input))
  );
  ipcMain.handle(IPC_CHANNELS.exportWorkspaceBackup, (_event, note?: string) => workspaceService.exportWorkspaceBackup(note));
  ipcMain.handle(IPC_CHANNELS.getRunDetail, (event, runId: string, projection: RunDetailProjection = 'full') => {
    const normalizedProjection = normalizedRunDetailProjection(projection);
    return withLatestRunDetailRequest(event, runId, (signal) =>
      timedMainIpcAsync('getRunDetail', { run: shortMetricId(runId), projection: runDetailProjectionMetricLabel(normalizedProjection) }, () =>
        workspaceService.getRunDetailForClient(runId, signal, normalizedProjection)
      )
    );
  });
  ipcMain.handle(IPC_CHANNELS.getRunHistoryPage, (_event, runId: string, beforeEventId: string, projection: RunDetailProjection = 'full') => {
    const normalizedProjection = normalizedRunDetailProjection(projection);
    return timedMainIpcAsync('getRunHistoryPage', { run: shortMetricId(runId), projection: runDetailProjectionMetricLabel(normalizedProjection) }, () =>
      workspaceService.getRunHistoryPageForClient(runId, beforeEventId, undefined, normalizedProjection)
    );
  });
  ipcMain.handle(IPC_CHANNELS.getRunDetailVersion, (_event, runId: string) =>
    timedMainIpcAsync('getRunDetailVersion', { run: shortMetricId(runId) }, () => workspaceService.getRunDetailVersionForClient(runId))
  );
  ipcMain.handle(IPC_CHANNELS.getRunDetailUpdate, (event, runId: string, cursor: RunDetailUpdateCursor, projection: RunDetailProjection = 'full') => {
    const normalizedProjection = normalizedRunDetailProjection(projection);
    return withLatestRunDetailRequest(event, runId, (signal) =>
      timedMainIpcAsync('getRunDetailUpdate', { run: shortMetricId(runId), projection: runDetailProjectionMetricLabel(normalizedProjection), afterTrace: cursor.afterTraceSequence, afterTranscript: cursor.afterTranscriptCount }, () =>
        workspaceService.getRunDetailUpdateForClient(runId, cursor, signal, normalizedProjection)
      )
    );
  });
  ipcMain.handle(IPC_CHANNELS.getRunMessageDetail, (_event, input: RunMessageDetailRequest) =>
    timedMainIpcAsync('getRunMessageDetail', { run: shortMetricId(input.runId), events: Array.isArray(input.traceEventIds) ? input.traceEventIds.length : 0 }, () =>
      workspaceService.getRunMessageDetailForClient(input)
    )
  );
  ipcMain.on(IPC_CHANNELS.cancelRunDetailRequests, (event, runId?: string) => cancelRunDetailRequest(event.sender.id, runId));
  ipcMain.handle(IPC_CHANNELS.searchSessionTranscripts, (_event, input: SessionTranscriptSearchInput) =>
    timedMainIpc('searchSessionTranscripts', { chars: input.query.length, limit: input.limit ?? 24, currentWorkspaceOnly: input.currentWorkspaceOnly !== false }, () =>
      workspaceService.searchSessionTranscripts(input)
    )
  );
  ipcMain.handle(IPC_CHANNELS.steerRun, (_event, action: SteeringAction) =>
    timedMainIpcAsync('steerRun', { type: action.type, run: shortMetricId(action.runId) }, () => workspaceService.steerRunForClient(action))
  );
  ipcMain.handle(IPC_CHANNELS.openNotification, (_event, notificationId: string) => workspaceService.openNotification(notificationId));
  ipcMain.handle(IPC_CHANNELS.dismissNotification, (_event, notificationId: string) => workspaceService.dismissNotification(notificationId));
  ipcMain.handle(IPC_CHANNELS.minimizeWindow, (event) => {
    windowForEvent(event)?.minimize();
  });
  ipcMain.handle(IPC_CHANNELS.toggleMaximizeWindow, (event) => {
    const window = windowForEvent(event);
    if (!window) return;
    if (window.isMaximized()) {
      window.unmaximize();
    } else {
      window.maximize();
    }
  });
  ipcMain.handle(IPC_CHANNELS.closeWindow, (event) => {
    windowForEvent(event)?.close();
  });
  ipcMain.handle(IPC_CHANNELS.getWindowChromeState, (event) => windowChromeState(windowForEvent(event)));
}

async function withLatestRunDetailRequest<T>(
  event: IpcMainInvokeEvent,
  runId: string,
  operation: (signal: AbortSignal) => Promise<T>
): Promise<{ canceled: true } | { canceled: false; value: T }> {
  const senderId = event.sender.id;
  const key = runDetailRequestKey(senderId, runId);
  cancelRunDetailRequest(senderId, runId);
  const controller = new AbortController();
  runDetailRequestControllers.set(key, controller);
  try {
    return { canceled: false, value: await operation(controller.signal) };
  } catch (error) {
    if (controller.signal.aborted) return { canceled: true };
    throw error;
  } finally {
    if (runDetailRequestControllers.get(key) === controller) {
      runDetailRequestControllers.delete(key);
    }
  }
}

function cancelRunDetailRequest(senderId: number, runId?: string): void {
  if (runId) {
    const key = runDetailRequestKey(senderId, runId);
    const controller = runDetailRequestControllers.get(key);
    if (!controller) return;
    runDetailRequestControllers.delete(key);
    controller.abort();
    return;
  }
  const prefix = `${senderId}:`;
  for (const [key, controller] of runDetailRequestControllers) {
    if (!key.startsWith(prefix)) continue;
    runDetailRequestControllers.delete(key);
    controller.abort();
  }
}

function runDetailRequestKey(senderId: number, runId: string): string {
  return `${senderId}:${runId}`;
}

if (!hasSingleInstanceLock) {
  app.quit();
} else {
  app.on('second-instance', reopenMainWindow);

  app.whenReady().then(async () => {
    applyAppIcon();
    setBealeDesktopRestartRequiredHandler((error) => {
      void promptForDesktopRestart(error);
    });
    const providerCredentialStore = new ProviderCredentialStore(
      join(app.getPath('userData'), 'provider-credentials.json'),
      {
        available: () => safeStorage.isEncryptionAvailable(),
        encrypt: (value) => safeStorage.encryptString(value),
        decrypt: (value) => safeStorage.decryptString(value)
      },
      { deferLoad: true }
    );
    let refreshProviderEnvironment = false;
    try {
      refreshProviderEnvironment = unlockProviderApiKeysForAppServerStartup(providerCredentialStore);
    } catch (error) {
      process.stderr.write(
        `Beale could not unlock saved provider API keys during Windows startup: ${error instanceof Error ? error.message : String(error)}\n`
      );
    }
    if (!await ensureAppServerContract(refreshProviderEnvironment)) return;
    workspaceService = new WorkspaceService(broadcastSnapshot, {
      providerCredentialStore,
      providerEnvironmentChanged: async () => {
        await restartBealeAppServer();
      }
    });
    iosDeviceCaptureService = new IosDeviceCaptureService(
      broadcastIosDeviceCaptureState,
      broadcastIosDeviceCaptureFrame
    );
    const browserBridge = new InAgentBrowserBridge(
      undefined,
      (update) => {
        if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(IPC_CHANNELS.browserContextsChanged, update);
      },
      async () => {
        const options = {
          type: 'warning' as const,
          title: 'Allow browser access in Beale?',
          message: 'Allow research sessions to control open Browser contexts?',
          detail: 'Full browser control can read page content, form values, cookies, session tokens, and network data. Complete sign-in and leave the password form before allowing access. This approval is saved on this machine and will not be requested again for later research sessions.',
          buttons: ['Cancel', 'Allow browser access'],
          defaultId: 0,
          cancelId: 0,
          noLink: true
        };
        const result = mainWindow && !mainWindow.isDestroyed()
          ? await dialog.showMessageBox(mainWindow, options)
          : await dialog.showMessageBox(options);
        return result.response === 1;
      },
      async (partition) => { await session.fromPartition(partition).clearStorageData(); }
    );
    try {
      await browserBridge.start();
      inAgentBrowserBridge = browserBridge;
    } catch (error) {
      browserBridge.stop();
      process.stderr.write(`Beale could not start the embedded browser bridge: ${error instanceof Error ? error.message : String(error)}\n`);
    }
    registerIpc();
    createWindow();
    if (app.isPackaged && !smokeTestMode) startDesktopUpdater();
    if (smokeTestMode) {
      setTimeout(() => app.quit(), 1500);
    }

    app.on('activate', reopenMainWindow);
  });

  app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') {
      app.quit();
    }
  });

  app.on('before-quit', () => {
    fleetBrowserViewer.close();
    inAgentBrowserBridge?.stop();
    iosDeviceCaptureService?.dispose();
    workspaceService?.dispose();
  });
}

function startDesktopUpdater(): void {
  desktopUpdater = new DesktopUpdater({
    backend: autoUpdater,
    currentVersion: app.getVersion(),
    stopLocalAppServer: stopBealeAppServerForUpdate,
    onChange: (state) => {
      if (mainWindow && !mainWindow.isDestroyed()) mainWindow.webContents.send(IPC_CHANNELS.desktopUpdateState, state);
    }
  });
  const check = (): void => { void desktopUpdater?.check(); };
  setTimeout(check, DESKTOP_UPDATE_FIRST_CHECK_DELAY_MS).unref();
  setInterval(check, DESKTOP_UPDATE_CHECK_INTERVAL_MS).unref();
}

function desktopUpdateState(): DesktopUpdateState {
  return desktopUpdater?.state() ?? {
    status: 'disabled',
    currentVersion: app.getVersion(),
    availableVersion: null,
    downloadPercent: null,
    message: null
  };
}

async function ensureAppServerContract(refreshProviderEnvironment = false): Promise<boolean> {
  try {
    if (refreshProviderEnvironment) await restartBealeAppServer();
    else await ensureBealeAppServerRunning();
    desktopRestartPrompt.compatible();
    return true;
  } catch (error) {
    if (!(error instanceof BealeDesktopRestartRequiredError)) {
      // Session startup retains its detailed error path if the server cannot
      // be launched for an environmental reason.
      return true;
    }
    return promptForDesktopRestart(error);
  }
}

function promptForDesktopRestart(error: BealeDesktopRestartRequiredError): Promise<boolean> {
  return desktopRestartPrompt.prompt(error.serverContractTimestamp);
}
