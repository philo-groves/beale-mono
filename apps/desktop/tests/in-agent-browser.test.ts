import { EventEmitter } from 'node:events';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { WebContents } from 'electron';
import { describe, expect, it } from 'vitest';
import { BrowserCdpSession } from '../../../packages/research-agent/src/browser-tools';
import { allowedBrowserCommand, allowedBrowserUrl, InAgentBrowserBridge } from '../src/main/inAgentBrowserBridge';
import { browserContextLabel, DEFAULT_BROWSER_CONTEXT } from '../src/shared/browserContexts';
import { browserNavigationUrl, BrowserSideView } from '../src/renderer/features/research/BrowserSideView';
import { ResearchSideViewChooser, ResearchSideViewTabs, researchSideNavigationReducer, researchSideViewsForProfile } from '../src/renderer/features/research/MemorySidePanel';

describe('in-agent browser', () => {
  it('offers Browser in the expanded chooser and tab picker', () => {
    expect(researchSideViewsForProfile(null)).toContain('browser:default');
    const chooser = renderToStaticMarkup(createElement(ResearchSideViewChooser, { onOpen: () => undefined }));
    expect(chooser).toContain('Browser');
    const tabs = renderToStaticMarkup(createElement(ResearchSideViewTabs, {
      activeView: 'browser:default',
      openViews: ['browser:default'],
      browserContextLabels: { 'browser:default': 'Default' },
      onActivate: () => undefined,
      onClose: () => undefined,
      onOpen: () => undefined,
      onRenameBrowserContext: async () => undefined
    }));
    expect(tabs).toContain('aria-selected="true"');
    expect(tabs).toContain('Close Browser Default');
    expect(tabs).toContain('class="research-browser-context-pill"');
    expect(tabs).toContain('aria-label="Rename Browser context Default"');
    const browser = renderToStaticMarkup(createElement(BrowserSideView, { visible: true, partition: DEFAULT_BROWSER_CONTEXT.partition, label: 'Default' }));
    expect(browser).toContain('aria-label="Browser URL"');
    expect(browser).toContain('class="research-browser-page"');
    expect(researchSideNavigationReducer({
      openViews: ['memory', 'browser:default', 'browser:context-example'], activeView: 'browser:context-example'
    }, { type: 'reset' })).toEqual({
      openViews: ['browser:default', 'browser:context-example'], activeView: 'browser:context-example'
    });
  });

  it('accepts web URLs and rejects local and privileged schemes', () => {
    expect(browserNavigationUrl('example.test/path')).toBe('https://example.test/path');
    expect(browserNavigationUrl('http://127.0.0.1:8080/')).toBe('http://127.0.0.1:8080/');
    expect(browserNavigationUrl('file:///example.txt')).toBeNull();
    expect(browserNavigationUrl('javascript:alert(1)')).toBeNull();
    expect(allowedBrowserUrl('about:blank')).toBe(true);
    expect(allowedBrowserUrl('https://example.test/')).toBe(true);
    expect(allowedBrowserUrl('file:///example.txt')).toBe(false);
    expect(allowedBrowserCommand('Page.navigate')).toBe(true);
    expect(allowedBrowserCommand('Target.getTargets')).toBe(false);
    expect(allowedBrowserCommand('Browser.getVersion')).toBe(false);
    expect(browserContextLabel('Admin')).toBe('Admin');
    expect(browserContextLabel('two words')).toBeNull();
    expect(browserContextLabel('Default Account')).toBeNull();
  });

  it('keeps labeled browser contexts and CDP connections isolated', async () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-browser-test-'));
    const discoveryFile = join(directory, 'browser.json');
    const acknowledgements: string[] = [];
    const updates: string[] = [];
    const bridge = new InAgentBrowserBridge(discoveryFile, (update) => {
      if (update.openedId) updates.push(update.openedId);
    }, async (sessionId) => { acknowledgements.push(sessionId); return sessionId !== 'session-example-denied'; });
    const fakeGuest = (id: number) => {
      const debuggerEvents = new EventEmitter();
      let attached = false;
      const guest = Object.assign(new EventEmitter(), {
        id,
        isDestroyed: () => false,
        getTitle: () => 'Example page',
        getURL: () => 'https://example.test/',
        setWindowOpenHandler: () => undefined,
        session: { clearStorageData: async () => undefined },
        debugger: Object.assign(debuggerEvents, {
          isAttached: () => attached,
          attach: () => { attached = true; },
          detach: () => { attached = false; },
          sendCommand: async (method: string, params: Record<string, unknown>) => ({ guestId: id, method, params })
        })
      }) as unknown as WebContents;
      return { guest, debuggerEvents };
    };
    const defaultGuest = fakeGuest(7);
    const adminGuest = fakeGuest(8);
    const browser = new BrowserCdpSession(async () => JSON.parse(readFileSync(discoveryFile, 'utf8')).endpoint as string, 'session-example-001');
    try {
      await bridge.start();
      bridge.attach('default', defaultGuest.guest);
      expect(await browser.contexts()).toEqual([{ id: 'default', label: 'Default' }]);
      const created = await browser.createContext('Admin');
      expect(created.label).toBe('Admin');
      expect(bridge.renameContext('default', 'Primary').label).toBe('Primary');
      expect(bridge.renameContext(created.id, 'Member').label).toBe('Member');
      await expect(Promise.resolve().then(() => bridge.renameContext(created.id, 'two words'))).rejects.toThrow('one word');
      await expect(Promise.resolve().then(() => bridge.renameContext(created.id, 'Primary'))).rejects.toThrow('already exists');
      await expect(browser.createContext('member')).rejects.toThrow('already exists');
      bridge.attach(created.id, adminGuest.guest);
      expect(await browser.openContext(created.id)).toEqual({ id: created.id, label: 'Member' });
      expect(updates).toEqual([created.id]);
      expect(bridge.listContexts()[0]?.partition).not.toBe(bridge.listContexts()[1]?.partition);
      expect(bridge.contextIdForPartition(bridge.listContexts()[1]?.partition ?? '')).toBe(created.id);
      expect(await browser.targets()).toMatchObject([
        { id: '7', description: 'Beale browser context: Primary', url: 'https://example.test/' },
        { id: '8', description: 'Beale browser context: Member', url: 'https://example.test/' }
      ]);
      const defaultConnection = await browser.connect();
      const adminConnection = await browser.connect(undefined, '8');
      expect(acknowledgements).toEqual(['session-example-001']);
      const denied = new BrowserCdpSession(async () => JSON.parse(readFileSync(discoveryFile, 'utf8')).endpoint as string, 'session-example-denied');
      await expect(denied.connect(undefined, '8')).rejects.toThrow('not acknowledged');
      expect(acknowledgements).toEqual(['session-example-001', 'session-example-denied']);
      expect(await browser.command(defaultConnection, 'Page.navigate', { url: 'https://example.test/next' })).toMatchObject({
        result: { guestId: 7, method: 'Page.navigate' }
      });
      expect(await browser.command(adminConnection, 'Page.navigate', { url: 'https://example.test/admin' })).toMatchObject({
        result: { guestId: 8, method: 'Page.navigate' }
      });
      expect(await browser.command(defaultConnection, 'Target.getTargets')).toMatchObject({ error: { code: -32601 } });
      expect(await browser.command(defaultConnection, 'Page.navigate', { url: 'file:///example.txt' })).toMatchObject({ error: { code: -32602 } });
      defaultGuest.debuggerEvents.emit('message', {}, 'Page.frameNavigated', { frame: { url: 'https://example.test/next' } });
      expect((await browser.events(defaultConnection, 50, 1000)).events).toMatchObject([{ method: 'Page.frameNavigated' }]);
      expect((await browser.events(adminConnection)).events).toEqual([]);
      const nextSession = new BrowserCdpSession(async () => JSON.parse(readFileSync(discoveryFile, 'utf8')).endpoint as string, 'session-example-002');
      expect(await nextSession.targets()).toHaveLength(2);
      const reusedConnection = await nextSession.connect(undefined, '8');
      expect(await nextSession.command(reusedConnection, 'DOM.getDocument')).toMatchObject({ result: { guestId: 8 } });
      expect(acknowledgements).toEqual(['session-example-001', 'session-example-denied', 'session-example-002']);
      await nextSession.cleanup();
      await expect(browser.createContext('two words')).rejects.toThrow('one word');
      await expect(browser.createContext('member')).rejects.toThrow('already exists');
      await expect(browser.closeContext('default')).rejects.toThrow('cannot be removed');
      await browser.closeContext(created.id);
      expect(await browser.contexts()).toEqual([{ id: 'default', label: 'Primary' }]);
      expect(await browser.targets()).toHaveLength(1);
      browser.disconnect(defaultConnection);
    } finally {
      await browser.cleanup();
      await bridge.stop();
      rmSync(directory, { recursive: true, force: true });
    }
  });

  it('closes a CDP socket without touching a destroyed browser guest', async () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-browser-lifecycle-test-'));
    const discoveryFile = join(directory, 'browser.json');
    const bridge = new InAgentBrowserBridge(discoveryFile, () => undefined, async () => true);
    const browser = new BrowserCdpSession(async () => JSON.parse(readFileSync(discoveryFile, 'utf8')).endpoint as string, 'session-example-lifecycle');
    const guestEvents = new EventEmitter();
    const debuggerEvents = new EventEmitter();
    let destroyed = false;
    let attached = false;
    let unsafeRemovals = 0;
    debuggerEvents.off = (eventName, listener) => {
      if (destroyed) {
        unsafeRemovals += 1;
        throw new Error('Object has been destroyed');
      }
      return EventEmitter.prototype.off.call(debuggerEvents, eventName, listener);
    };
    const guest = Object.assign(guestEvents, {
      id: 17,
      isDestroyed: () => destroyed,
      getTitle: () => 'Example page',
      getURL: () => 'https://example.test/',
      setWindowOpenHandler: () => undefined,
      debugger: Object.assign(debuggerEvents, {
        isAttached: () => attached,
        attach: () => { attached = true; },
        detach: () => { attached = false; },
        sendCommand: () => { throw new Error('Object has been destroyed'); }
      })
    }) as unknown as WebContents;

    try {
      await bridge.start();
      bridge.attach('default', guest);
      const connection = await browser.connect();
      await expect(browser.command(connection, 'Runtime.evaluate')).resolves.toMatchObject({
        error: { code: -32000, message: 'Object has been destroyed' }
      });
      const closed = browser.events(connection, 1, 1000);
      destroyed = true;
      guestEvents.emit('destroyed');
      await closed;
      expect(unsafeRemovals).toBe(0);
    } finally {
      await browser.cleanup();
      bridge.stop();
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
