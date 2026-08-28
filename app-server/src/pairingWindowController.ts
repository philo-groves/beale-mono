export interface PairingWindowSurface {
  isDestroyed(): boolean;
  show(): void;
  focus(): void;
  destroy(): void;
  on(event: 'closed', listener: () => void): unknown;
}

/**
 * Keeps the first reveal synchronous with the tray click. Windows only grants
 * foreground activation to that user gesture; waiting for QR rendering first
 * can leave the new window hidden behind other applications until a second
 * tray click focuses it.
 */
export class PairingWindowController<TWindow extends PairingWindowSurface> {
  private window: TWindow | null = null;

  public async show(
    createWindow: () => TWindow,
    loadWindow: (window: TWindow) => Promise<void>
  ): Promise<void> {
    if (this.window && !this.window.isDestroyed()) {
      reveal(this.window);
      return;
    }

    const window = createWindow();
    this.window = window;
    window.on('closed', () => {
      if (this.window === window) this.window = null;
    });

    // This must happen before the first await so Windows still associates the
    // BrowserWindow activation with the tray menu click.
    reveal(window);

    try {
      await loadWindow(window);
      if (!window.isDestroyed()) reveal(window);
    } catch (error) {
      if (!window.isDestroyed()) window.destroy();
      throw error;
    }
  }
}

function reveal(window: PairingWindowSurface): void {
  window.show();
  window.focus();
}
