export type FleetBrowserUpdate =
  | { runId: string; type: 'browser.ready'; url: string; width: number; height: number }
  | { runId: string; type: 'browser.frame'; mime: 'image/jpeg'; data: string; width: number; height: number }
  | { runId: string; type: 'browser.location'; url: string }
  | { runId: string; type: 'browser.ack'; id: number }
  | { runId: string; type: 'browser.error'; message: string }
  | { runId: string; type: 'browser.disconnected' };

export type FleetBrowserInput =
  | { type: 'browser.navigate'; url: string }
  | { type: 'browser.mouse'; eventType: 'mousePressed' | 'mouseReleased' | 'mouseMoved' | 'mouseWheel'; x: number; y: number;
      button?: 'left' | 'middle' | 'right' | 'none'; buttons?: number; clickCount?: number; deltaX?: number; deltaY?: number }
  | { type: 'browser.key'; eventType: 'keyDown' | 'keyUp' | 'rawKeyDown' | 'char'; key: string; code: string;
      text?: string; modifiers?: number }
  | { type: 'browser.text'; text: string }
  | { type: 'browser.release' };
