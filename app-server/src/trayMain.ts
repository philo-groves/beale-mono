import { app, BrowserWindow, clipboard, dialog, Menu, nativeImage, powerSaveBlocker, Tray } from 'electron';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { extname, join } from 'node:path';
import QRCode from 'qrcode';
import {
  acquireDiscoveryLock,
  defaultDiscoveryPath,
  isProcessAlive,
  readDiscoveryRecord,
  releaseDiscoveryLock,
  type AppServerDiscoveryRecord
} from './discovery.js';
import { startAppServer, type AppServerHandle, type AppServerOptions, type SessionCatalogEntry } from './appServer.js';
import { createAppServerPairingPayload } from './pairing.js';
import { PairingWindowController } from './pairingWindowController.js';
import { readPersistedRemoteAccessLaunchOptions } from './remoteAccessConfig.js';

const CHECK_EXIT_DELAY_MS = 1_500;
const MACOS_TRAY_ICON_SIZE = 18;
const MACOS_TRAY_ICON_SCALE_FACTOR = 2;

let server: AppServerHandle | null = null;
let tray: Tray | null = null;
const pairingWindowController = new PairingWindowController<BrowserWindow>();
let shuttingDown = false;
let discoveryLockHeld = false;
let discoveryMonitor: NodeJS.Timeout | null = null;
let appSuspensionBlockerId: number | null = null;

const checkOnly = process.argv.includes('--check');
const stateFile = process.env.BEALE_APP_SERVER_STATE_FILE?.trim() || defaultDiscoveryPath();

// The tray host and Beale Desktop are separate Electron applications. The
// branded Electron runtime otherwise gives both processes the same default
// userData directory, allowing the tray host's Chromium profile lock to make
// Desktop BrowserWindow creation fail after the app-server starts.
app.setPath('userData', join(app.getPath('appData'), 'beale-app-server'));

if (process.platform === 'darwin') {
  // Establish tray-only activation before Electron becomes ready so LaunchServices
  // never presents the background host in the Dock during startup or replacement.
  app.setActivationPolicy('accessory');
  app.dock?.hide();
}

// The discovery lock is the cross-host startup authority. Do not layer
// Electron's single-instance lock on top of it: a legacy or orphaned tray
// process can retain that app-wide lock after losing discovery ownership and
// prevent a replacement host from ever reaching this recoverable gate.
void app.whenReady().then(async () => {
  if (!acquireDiscoveryLock(stateFile, process.pid)) {
    // Another host is starting or still completing shutdown. This is a normal
    // duplicate-launch race; never hold the process open with a modal dialog.
    app.exit(0);
    return;
  }
  discoveryLockHeld = true;

  const existing = readDiscoveryRecord(stateFile);
  if (!checkOnly && existing && isProcessAlive(existing.pid)) {
    releaseTrayDiscoveryLock();
    app.exit(1);
    return;
  }

  const persistedRemoteAccess = readPersistedRemoteAccessLaunchOptions();
  const host = process.env.BEALE_APP_SERVER_HOST?.trim() || persistedRemoteAccess?.host || '127.0.0.1';
  const portEnv = Number.parseInt(process.env.BEALE_APP_SERVER_PORT ?? '', 10);
  const port = Number.isInteger(portEnv) && portEnv > 0 ? portEnv : persistedRemoteAccess?.port ?? 0;
  const operatorToken = process.env.BEALE_APP_SERVER_TOKEN?.trim() || undefined;
  const publicUrl = process.env.BEALE_APP_SERVER_PUBLIC_URL?.trim() || persistedRemoteAccess?.publicUrl;

  const serverOptions: AppServerOptions = {
    host,
    port,
    hostMode: 'tray',
    recoverInterruptedOnStart: true,
    ...(publicUrl ? { publicUrl } : {}),
    discoveryFile: stateFile,
    onChange: () => refreshTrayMenu(),
    onShutdownRequested: () => app.quit()
  };
  if (operatorToken) serverOptions.operatorToken = operatorToken;

  try {
    server = await startAppServer(serverOptions);
  } catch (error) {
    releaseTrayDiscoveryLock();
    process.stderr.write(`The Beale app-server failed to start: ${error instanceof Error ? error.message : String(error)}\n`);
    app.exit(1);
    return;
  }
  appSuspensionBlockerId = powerSaveBlocker.start('prevent-app-suspension');

  await createTray();
  refreshTrayMenu();
  discoveryMonitor = setInterval(() => {
    const discovery = readDiscoveryRecord(stateFile);
    if (!discovery || discovery.pid !== process.pid) app.quit();
  }, 1_000);
  discoveryMonitor.unref();

  if (checkOnly) {
    process.stdout.write(`Beale App Server listening at ${server.url} (--check)\n`);
    setTimeout(() => app.quit(), CHECK_EXIT_DELAY_MS);
  }
});

app.on('window-all-closed', () => {
  // Tray-only application: never quit because windows closed.
});

app.on('before-quit', (event) => {
  if (discoveryMonitor) {
    clearInterval(discoveryMonitor);
    discoveryMonitor = null;
  }
  if (shuttingDown) return;
  if (!server) {
    releaseAppSuspensionBlocker();
    releaseTrayDiscoveryLock();
    return;
  }
  event.preventDefault();
  shuttingDown = true;
  const closing = server.close();
  server = null;
  void closing.then(
    () => {
      releaseAppSuspensionBlocker();
      releaseTrayDiscoveryLock();
      app.exit(0);
    },
    () => {
      releaseAppSuspensionBlocker();
      releaseTrayDiscoveryLock();
      app.exit(1);
    }
  );
});

function releaseAppSuspensionBlocker(): void {
  if (appSuspensionBlockerId === null) return;
  if (powerSaveBlocker.isStarted(appSuspensionBlockerId)) {
    powerSaveBlocker.stop(appSuspensionBlockerId);
  }
  appSuspensionBlockerId = null;
}

function releaseTrayDiscoveryLock(): void {
  if (!discoveryLockHeld) return;
  releaseDiscoveryLock(stateFile, process.pid);
  discoveryLockHeld = false;
}

async function createTray(): Promise<void> {
  const icon = await createTrayIcon();
  tray = new Tray(icon);
  tray.setToolTip('Beale App Server');
}

async function createTrayIcon(): Promise<Electron.NativeImage> {
  for (const iconPath of trayIconPaths()) {
    if (process.platform === 'darwin' && extname(iconPath).toLowerCase() === '.svg') {
      try {
        const icon = await createMacosSvgTrayIcon(iconPath);
        if (!icon.isEmpty()) return icon;
      } catch (error) {
        process.stderr.write(
          `Beale App Server could not rasterize the macOS menu-bar icon at ${iconPath}: ${error instanceof Error ? error.message : String(error)}\n`
        );
      }
      continue;
    }

    const source = nativeImage.createFromPath(iconPath);
    if (source.isEmpty()) continue;
    return createBitmapTrayIcon(source);
  }
  return nativeImage.createEmpty();
}

function createBitmapTrayIcon(source: Electron.NativeImage): Electron.NativeImage {

  // The source artwork is landscape. Width-only scaling produced a roughly
  // 16x9 image on Windows, which was effectively invisible in the tray.
  const size = source.getSize();
  const cropSize = Math.min(size.width, size.height);
  const square = source.crop({
    x: Math.max(0, Math.floor((size.width - cropSize) / 2)),
    y: Math.max(0, Math.floor((size.height - cropSize) / 2)),
    width: cropSize,
    height: cropSize
  });
  const targetSize = process.platform === 'win32' ? 32 : MACOS_TRAY_ICON_SIZE;
  const icon = square.resize({ width: targetSize, height: targetSize, quality: 'best' });
  if (process.platform === 'darwin') {
    // macOS menu-bar artwork is a template image: the system supplies the
    // foreground color so the icon remains legible in either appearance and
    // while the menu is highlighted.
    icon.setTemplateImage(true);
  }
  return icon;
}

async function createMacosSvgTrayIcon(iconPath: string): Promise<Electron.NativeImage> {
  const svg = readFileSync(iconPath, 'utf8');
  if (!/<svg\b/iu.test(svg)) throw new Error('the file does not contain SVG artwork');

  const rasterSize = MACOS_TRAY_ICON_SIZE * MACOS_TRAY_ICON_SCALE_FACTOR;
  const renderer = new BrowserWindow({
    width: rasterSize,
    height: rasterSize,
    useContentSize: true,
    show: false,
    frame: false,
    transparent: true,
    backgroundColor: '#00000000',
    webPreferences: {
      backgroundThrottling: false,
      contextIsolation: true,
      nodeIntegration: false,
      offscreen: true,
      sandbox: true
    }
  });
  renderer.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  renderer.webContents.on('will-navigate', (event) => event.preventDefault());

  try {
    const html = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
  <style>
    html, body { width: 100%; height: 100%; margin: 0; overflow: hidden; background: transparent; }
    svg { display: block; width: 100%; height: 100%; }
  </style>
</head>
<body>${svg}</body>
</html>`;
    await renderer.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(html)}`);
    const capture = await renderer.webContents.capturePage({
      x: 0,
      y: 0,
      width: rasterSize,
      height: rasterSize
    });
    if (capture.isEmpty()) throw new Error('Chromium returned an empty raster');

    const icon = nativeImage.createEmpty();
    icon.addRepresentation({
      scaleFactor: 1,
      buffer: capture.resize({
        width: MACOS_TRAY_ICON_SIZE,
        height: MACOS_TRAY_ICON_SIZE,
        quality: 'best'
      }).toPNG()
    });
    icon.addRepresentation({
      scaleFactor: MACOS_TRAY_ICON_SCALE_FACTOR,
      buffer: capture.toPNG()
    });
    icon.setTemplateImage(true);
    return icon;
  } finally {
    renderer.destroy();
  }
}

function trayIconPaths(): string[] {
  const candidates = [
    process.env.BEALE_APP_SERVER_ICON?.trim(),
    ...(process.platform === 'darwin'
      ? [
          join(app.getAppPath(), 'resources', 'MenuBarIcon.svg'),
          join(app.getAppPath(), '..', 'apps', 'desktop', 'resources', 'MenuBarIcon.svg'),
          fileURLToPath(new URL('../../apps/desktop/resources/MenuBarIcon.svg', import.meta.url)),
          join(process.resourcesPath, 'MenuBarIcon.svg'),
          join(process.resourcesPath, 'resources', 'MenuBarIcon.svg')
        ]
      : []),
    join(app.getAppPath(), 'resources', 'icon.png'),
    fileURLToPath(new URL('../resources/icon.png', import.meta.url)),
    join(process.resourcesPath, 'icon.png'),
    join(process.resourcesPath, 'resources', 'icon.png')
  ].filter((candidate): candidate is string => Boolean(candidate));
  return [...new Set(candidates)].filter((candidate) => existsSync(candidate));
}

function refreshTrayMenu(): void {
  if (!tray || !server) return;
  const sessions: SessionCatalogEntry[] = server.listSessions();
  const activeSessions = sessions.filter((session) => session.state === 'starting' || session.state === 'running');

  const menu = Menu.buildFromTemplate([
    { label: `Beale App Server — ${server.url}`, enabled: false },
    {
      label: sessionSummaryLine(activeSessions),
      enabled: false
    },
    { type: 'separator' },
    {
      label: 'Copy Endpoint URL',
      click: () => clipboard.writeText(server!.url)
    },
    {
      label: 'Copy Operator Token',
      click: () => clipboard.writeText(server!.operatorToken)
    },
    {
      label: 'Show QR Code',
      click: () => {
        void showPairingWindow().catch((error: unknown) => {
          dialog.showErrorBox(
            'Beale App Server',
            `The pairing QR code could not be shown: ${error instanceof Error ? error.message : String(error)}`
          );
        });
      }
    },
    ...(activeSessions.length > 0 ? [{ type: 'separator' as const }, ...activeSessionItems(activeSessions)] : []),
    { type: 'separator' },
    {
      label: process.platform === 'darwin' ? 'Quit Beale App Server' : 'Quit',
      click: () => app.quit()
    }
  ]);
  tray.setContextMenu(menu);
}

async function showPairingWindow(): Promise<void> {
  if (!server) return;
  const activeServer = server;
  await pairingWindowController.show(
    () => createPairingBrowserWindow(),
    async (window) => {
      const payload = createAppServerPairingPayload(activeServer.url, activeServer.operatorToken);
      const qrSvg = await QRCode.toString(payload, {
        type: 'svg',
        errorCorrectionLevel: 'M',
        margin: 2,
        width: 280,
        color: { dark: '#0d0d0dff', light: '#ffffffff' }
      });
      if (window.isDestroyed()) return;
      const endpoint = escapeHtml(activeServer.url);
      const secureEndpoint = activeServer.url.startsWith('https://');
      const endpointNote = secureEndpoint
        ? 'Open Beale on your iPhone and choose Scan QR Code.'
        : 'This endpoint is not reachable from iPhone. Configure an HTTPS Tailscale Serve public URL, then restart the app-server.';
      const noteClass = secureEndpoint ? 'note' : 'note warning';
      const html = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Connect Beale iOS</title>
  <style>
    :root { color-scheme: dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    body { margin: 0; background: #151515; color: #f2f2f2; }
    main { display: flex; min-height: 100vh; box-sizing: border-box; flex-direction: column; align-items: center; padding: 28px 32px; text-align: center; }
    h1 { margin: 0 0 8px; font-size: 20px; font-weight: 600; }
    .note { margin: 0 0 18px; color: #aaa; font-size: 13px; line-height: 1.4; }
    .warning { color: #ffb454; }
    .qr { width: 280px; height: 280px; padding: 10px; border-radius: 16px; background: white; box-sizing: content-box; }
    .qr svg { display: block; width: 100%; height: 100%; }
    .endpoint { max-width: 330px; margin-top: 18px; color: #d3d3d3; font-size: 12px; overflow-wrap: anywhere; }
    .security { margin: 14px 0 0; color: #888; font-size: 11px; line-height: 1.35; }
  </style>
</head>
<body>
  <main>
    <h1>Connect Beale iOS</h1>
    <p class="${noteClass}">${endpointNote}</p>
    <div class="qr">${qrSvg}</div>
    <div class="endpoint">${endpoint}</div>
    <p class="security">This code contains the operator token. Only scan it with a trusted device on your tailnet.</p>
  </main>
</body>
</html>`;
      await window.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(html)}`);
    }
  );
}

function createPairingBrowserWindow(): BrowserWindow {
  const window = new BrowserWindow({
    width: 430,
    height: 570,
    title: 'Connect Beale iOS',
    backgroundColor: '#151515',
    resizable: false,
    maximizable: false,
    fullscreenable: false,
    show: false,
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true
    }
  });
  window.setMenuBarVisibility(false);
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  window.webContents.on('will-navigate', (event) => event.preventDefault());
  return window;
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/gu, (character) => ({
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#39;'
  })[character]!);
}

function activeSessionItems(sessions: SessionCatalogEntry[]): Electron.MenuItemConstructorOptions[] {
  return sessions.slice(0, 10).map((session) => ({
    label: `${session.sessionId} (${session.clientConnected ? 'client attached' : 'waiting'})`,
    enabled: false
  }));
}

function sessionSummaryLine(active: SessionCatalogEntry[]): string {
  if (active.length === 0) return 'No active sessions';
  return `${active.length} active session${active.length === 1 ? '' : 's'}`;
}
