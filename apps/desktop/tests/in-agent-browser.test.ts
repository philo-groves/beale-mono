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
import { ResearchSidePanel, ResearchSideViewChooser, ResearchSideViewTabs, availableResearchSideViews, researchSideNavigationReducer, researchSideViewsForProfile } from '../src/renderer/features/research/MemorySidePanel';

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
    const browser = renderToStaticMarkup(createElement(BrowserSideView, { visible: true, partition: DEFAULT_BROWSER_CONTEXT.partition, label: 'Default', lastUrl: DEFAULT_BROWSER_CONTEXT.lastUrl }));
    expect(browser).toContain('aria-label="Browser URL"');
    expect(browser).toContain('class="research-browser-page"');
    expect(researchSideNavigationReducer({
      openViews: ['memory', 'browser:default', 'browser:context-example'], activeView: 'browser:context-example'
    }, { type: 'reset' })).toEqual({
      openViews: ['browser:default', 'browser:context-example'], activeView: 'browser:context-example'
    });
    const closedTab = researchSideNavigationReducer({
      openViews: ['browser:default', 'browser:context-example'], activeView: 'browser:context-example'
    }, { type: 'close', view: 'browser:context-example' });
    expect(availableResearchSideViews(closedTab.openViews, ['browser:default', 'browser:context-example']))
      .toContain('browser:context-example');
  });

  it('offers a way back to Browser after the last tab closes and the sidebar collapses', () => {
    const closedTab = researchSideNavigationReducer({
      openViews: ['browser:default'], activeView: 'browser:default'
    }, { type: 'close', view: 'browser:default' });
    expect(closedTab).toEqual({ openViews: [], activeView: null });
    for (const viewSpace of ['session', 'workspace'] as const) {
      const summary = renderToStaticMarkup(createElement(ResearchSidePanel, {
        detail: null,
        events: [],
        memory: null,
        providerModelCatalog: [],
        runId: 'run-example',
        runStatus: null,
        selectedRunbook: null,
        selectedRunbookDocument: null,
        runbookLoading: false,
        runbookError: null,
        selectedSubagentPath: null,
        selectedRunbookId: null,
        searchHighlightQuery: '',
        onOpenRunbook: () => undefined,
        onSelectSubagent: () => undefined,
        onBackToRunbooks: () => undefined,
        onBackToSubagents: () => undefined,
        expanded: false,
        viewSpace
      }));
      expect(summary).toContain('aria-label="Open Browser Default"');
    }
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
        close: () => undefined,
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

  it('restores context labels, page locations, and isolated storage after restart', async () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-browser-persistence-test-'));
    const discoveryFile = join(directory, 'browser.json');
    const first = new InAgentBrowserBridge(discoveryFile);
    const guestEvents = new EventEmitter();
    let currentUrl = 'about:blank';
    const guest = Object.assign(guestEvents, {
      id: 21,
      isDestroyed: () => false,
      getURL: () => currentUrl,
      setWindowOpenHandler: () => undefined,
      debugger: { isAttached: () => false }
    }) as unknown as WebContents;
    try {
      const created = first.createContext('AccountOne');
      first.renameContext('default', 'Primary');
      first.attach(created.id, guest);
      currentUrl = 'https://example.test/account?page=2';
      guestEvents.emit('did-navigate');
      currentUrl = 'https://example.test/account?page=3';
      guestEvents.emit('did-navigate-in-page');
      const saved = new InAgentBrowserBridge(discoveryFile, () => undefined, async () => false, async (partition) => {
        expect(partition).toBe(created.partition);
      });
      expect(saved.listContexts()).toMatchObject([
        { id: 'default', label: 'Primary', lastUrl: 'about:blank' },
        { id: created.id, label: 'AccountOne', lastUrl: currentUrl }
      ]);
      expect(saved.listContexts().every((context) => context.partition.startsWith('persist:'))).toBe(true);
      const restoredPage = renderToStaticMarkup(createElement(BrowserSideView, {
        visible: true, partition: created.partition, label: created.label, lastUrl: currentUrl
      }));
      expect(restoredPage).toContain('value="https://example.test/account?page=3"');
      await saved.removeContext(created.id);
      expect(new InAgentBrowserBridge(discoveryFile).listContexts()).toHaveLength(1);
    } finally {
      first.stop();
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
