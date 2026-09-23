import { randomUUID } from 'node:crypto';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import puppeteer from 'puppeteer-core';

const read = annotation('read', ['inspect'], 'never');
const write = annotation('write', ['experiment'], 'always');
const string = (maxLength) => ({ type: 'string', minLength: 1, maxLength });
const schema = (properties, required = []) => ({ type: 'object', properties, required, additionalProperties: false });
const tabId = string(80);
const observationId = string(80);
const TOOLS = [
  { name: 'list_tabs', description: 'List tabs in the isolated Beale browser session.', inputSchema: schema({}), annotations: read },
  { name: 'open', description: 'Open an HTTP(S) page in a new isolated browser tab using WebDriver BiDi.', inputSchema: schema({ url: string(4096), browser: { type: 'string', enum: ['chrome', 'firefox'], default: 'chrome' } }, ['url']), annotations: write },
  { name: 'navigate', description: 'Navigate one Beale browser tab to an HTTP(S) URL.', inputSchema: schema({ tabId, url: string(4096) }, ['tabId', 'url']), annotations: write },
  { name: 'observe', description: 'Read a bounded page text and interactive element list. Each element receives a short-lived observation ID for click or fill.', inputSchema: schema({ tabId }, ['tabId']), annotations: read },
  { name: 'click', description: 'Click one freshly observed element in a Beale browser tab.', inputSchema: schema({ tabId, observationId }, ['tabId', 'observationId']), annotations: write },
  { name: 'fill', description: 'Fill one freshly observed input or textarea. Password fields are excluded from observation.', inputSchema: schema({ tabId, observationId, text: { type: 'string', maxLength: 16384 } }, ['tabId', 'observationId', 'text']), annotations: write },
  { name: 'capture', description: 'Capture the visible page as a bounded PNG image.', inputSchema: schema({ tabId }, ['tabId']), annotations: read },
  { name: 'close', description: 'Close a Beale browser tab; closing the last tab stops the isolated browser.', inputSchema: schema({ tabId }, ['tabId']), annotations: write }
];

let browser;
let browserKind;
let profileDirectory;
let inputBuffer = '';
let sequence = Promise.resolve();
const tabs = new Map();
const observations = new Map();
const MAX_TABS = 8;
const MAX_ELEMENTS = 80;
const MAX_SCREENSHOT_BYTES = 10 * 1024 * 1024;
const OBSERVATION_TTL_MS = 30_000;

process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  inputBuffer += chunk;
  if (inputBuffer.length > 2_000_000) {
    inputBuffer = '';
    return;
  }
  while (inputBuffer.includes('\n')) {
    const index = inputBuffer.indexOf('\n');
    const line = inputBuffer.slice(0, index).trim();
    inputBuffer = inputBuffer.slice(index + 1);
    if (line) {
      sequence = sequence.then(() => handle(line)).catch((error) => {
        console.error(error instanceof Error ? error.message : String(error));
      });
    }
  }
});
process.stdin.on('end', () => { void sequence.finally(() => stopBrowser()); });

async function handle(line) {
  let request;
  try {
    request = JSON.parse(line);
  } catch {
    send({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Invalid JSON-RPC payload.' } });
    return;
  }
  if (typeof request.method !== 'string' || request.method.startsWith('notifications/')) return;
  const { id, method, params } = request;
  if (method === 'initialize') {
    send({ jsonrpc: '2.0', id, result: { protocolVersion: params?.protocolVersion ?? '2025-11-25', capabilities: { tools: {} }, serverInfo: { name: 'beale-browser-use', version: '0.1.0' } } });
    return;
  }
  if (method === 'ping') return send({ jsonrpc: '2.0', id, result: {} });
  if (method === 'tools/list') return send({ jsonrpc: '2.0', id, result: { tools: TOOLS } });
  if (method !== 'tools/call') return send({ jsonrpc: '2.0', id: id ?? null, error: { code: -32601, message: 'Unsupported method.' } });
  const name = params?.name;
  const args = params?.arguments;
  if (!TOOLS.some((tool) => tool.name === name) || !args || typeof args !== 'object' || Array.isArray(args)) {
    return toolError(id, 'Unknown tool or invalid arguments.');
  }
  try {
    const result = await call(name, args);
    send({ jsonrpc: '2.0', id, result: result?.content ? result : textResult(result) });
  } catch (error) {
    toolError(id, error instanceof Error ? error.message : String(error));
  }
}

async function call(name, args) {
  pruneObservations();
  if (name === 'list_tabs') return tabList();
  if (name === 'open') {
    const url = httpUrl(args.url);
    const kind = args.browser ?? 'chrome';
    if (!['chrome', 'firefox'].includes(kind)) throw new Error('Unsupported browser.');
    if (tabs.size >= MAX_TABS) throw new Error('Browser tab limit reached.');
    await ensureBrowser(kind);
    const page = await browser.newPage();
    page.on('dialog', (dialog) => { void dialog.dismiss(); });
    const id = randomUUID();
    tabs.set(id, page);
    try {
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30_000 });
      return { tabId: id, url: page.url(), title: await page.title() };
    } catch (error) {
      tabs.delete(id);
      await page.close();
      if (tabs.size === 0) await stopBrowser();
      throw error;
    }
  }
  const page = requireTab(args.tabId);
  if (name === 'navigate') {
    const url = httpUrl(args.url);
    clearObservations(args.tabId);
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 30_000 });
    return { tabId: args.tabId, url: page.url(), title: await page.title() };
  }
  if (name === 'observe') return observe(args.tabId, page);
  if (name === 'click' || name === 'fill') {
    const lease = requireObservation(args.tabId, args.observationId, page);
    observations.delete(args.observationId);
    try {
      if (name === 'click') await lease.handle.click();
      else {
        if (!lease.fillable) throw new Error('Observed element is not fillable.');
        if (typeof args.text !== 'string' || args.text.length > 16_384) throw new Error('Invalid fill text.');
        await lease.handle.click();
        const modifier = process.platform === 'darwin' ? 'Meta' : 'Control';
        await page.keyboard.down(modifier);
        try { await page.keyboard.press('A'); } finally { await page.keyboard.up(modifier); }
        await page.keyboard.press('Backspace');
        await lease.handle.type(args.text);
      }
      return { tabId: args.tabId, url: page.url(), action: name };
    } finally {
      await lease.handle.dispose();
    }
  }
  if (name === 'capture') {
    const png = Buffer.from(await page.screenshot({ type: 'png' }));
    if (png.length > MAX_SCREENSHOT_BYTES) throw new Error('Screenshot exceeds 10 MiB.');
    return { content: [{ type: 'image', data: png.toString('base64'), mimeType: 'image/png' }] };
  }
  if (name === 'close') {
    clearObservations(args.tabId);
    tabs.delete(args.tabId);
    await page.close();
    if (tabs.size === 0) await stopBrowser();
    return { closed: args.tabId };
  }
  throw new Error('Unknown tool.');
}

async function ensureBrowser(kind) {
  if (browser) {
    if (browserKind !== kind) throw new Error(`Browser session already uses ${browserKind}; close its tabs before selecting ${kind}.`);
    return;
  }
  profileDirectory = await mkdtemp(join(tmpdir(), 'beale-browser-'));
  try {
    browser = await puppeteer.launch({
      browser: kind,
      protocol: 'webDriverBiDi',
      ...(process.env.BEALE_BROWSER_EXECUTABLE_PATH
        ? { executablePath: process.env.BEALE_BROWSER_EXECUTABLE_PATH }
        : kind === 'chrome' ? { channel: 'chrome' } : {}),
      headless: true,
      userDataDir: profileDirectory,
      timeout: 30_000
    });
    browserKind = kind;
  } catch {
    await rm(profileDirectory, { recursive: true, force: true });
    profileDirectory = undefined;
    throw new Error(`Could not launch ${kind} with WebDriver BiDi. Install a supported browser or set BEALE_BROWSER_EXECUTABLE_PATH.`);
  }
}

async function stopBrowser() {
  clearObservations();
  const closing = browser;
  const directory = profileDirectory;
  browser = undefined;
  browserKind = undefined;
  profileDirectory = undefined;
  try { await closing?.close(); } finally {
    if (directory) await rm(directory, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
  }
}

async function observe(id, page) {
  clearObservations(id);
  const text = await page.evaluate(() => (document.body?.innerText ?? '').slice(0, 12_000));
  const handles = await page.$$('a[href], button, input:not([type="password"]), textarea, select, [role="button"], [role="link"]');
  const elements = [];
  for (const handle of handles) {
    if (elements.length >= MAX_ELEMENTS) { await handle.dispose(); continue; }
    const detail = await handle.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      if (!rect.width || !rect.height || style.visibility === 'hidden' || style.display === 'none') return null;
      const tag = element.tagName.toLowerCase();
      const inputType = element.getAttribute('type')?.toLowerCase();
      if (inputType === 'password') return null;
      return {
        role: element.getAttribute('role') ?? tag,
        text: (element.getAttribute('aria-label') || element.labels?.[0]?.innerText || element.innerText || element.getAttribute('placeholder') || '').trim().slice(0, 200),
        href: tag === 'a' ? element.href.slice(0, 1000) : undefined,
        fillable: tag === 'textarea' || (tag === 'input' && !['checkbox', 'radio', 'button', 'submit', 'file', 'hidden'].includes(inputType))
      };
    }).catch(() => null);
    if (!detail) { await handle.dispose(); continue; }
    const observationId = randomUUID();
    observations.set(observationId, { tabId: id, handle, page, url: page.url(), expiresAt: Date.now() + OBSERVATION_TTL_MS, fillable: detail.fillable });
    elements.push({ observationId, ...detail });
  }
  return { tabId: id, url: page.url(), title: await page.title(), text, elements, truncated: handles.length > MAX_ELEMENTS || text.length === 12_000 };
}

function requireObservation(tabId, id, page) {
  if (typeof id !== 'string') throw new Error('Observation ID is required.');
  const lease = observations.get(id);
  if (!lease || lease.tabId !== tabId || lease.page !== page || lease.url !== page.url() || lease.expiresAt < Date.now()) {
    throw new Error('Observation expired or page changed; observe the page again.');
  }
  return lease;
}

function requireTab(id) {
  if (typeof id !== 'string' || !tabs.has(id)) throw new Error('Unknown Beale browser tab.');
  return tabs.get(id);
}

function clearObservations(tabId) {
  for (const [id, lease] of observations) {
    if (tabId && lease.tabId !== tabId) continue;
    observations.delete(id);
    void lease.handle.dispose().catch(() => {});
  }
}

function pruneObservations() {
  for (const [id, lease] of observations) {
    if (lease.expiresAt >= Date.now()) continue;
    observations.delete(id);
    void lease.handle.dispose().catch(() => {});
  }
}

function httpUrl(value) {
  if (typeof value !== 'string' || value.length > 4096) throw new Error('HTTP(S) URL required.');
  const url = new URL(value);
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) throw new Error('Only HTTP(S) URLs without embedded credentials are supported.');
  return url.href;
}

function tabList() {
  return { tabs: [...tabs].map(([tabId, page]) => ({ tabId, url: page.url() })) };
}

function annotation(sideEffects, actionClasses, confirmation) {
  return { readOnlyHint: sideEffects === 'read', destructiveHint: sideEffects === 'write', openWorldHint: true,
    'beale.io/tool': { actionClasses, sideEffects, requiredPermissions: [`browser-use:${sideEffects === 'read' ? 'observe' : 'mutate'}`], confirmation } };
}

function textResult(value) { return { content: [{ type: 'text', text: JSON.stringify(value) }] }; }
function toolError(id, message) { send({ jsonrpc: '2.0', id, result: { isError: true, content: [{ type: 'text', text: message }] } }); }
function send(value) { process.stdout.write(`${JSON.stringify(value)}\n`); }

process.on('SIGTERM', () => { void stopBrowser().finally(() => process.exit(0)); });
process.on('SIGINT', () => { void stopBrowser().finally(() => process.exit(0)); });
