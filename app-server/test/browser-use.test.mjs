import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';

const serverPath = fileURLToPath(new URL('../resources/agent-plugins/beale-browser-use/server.mjs', import.meta.url));

test('browser-use plugin advertises bounded BiDi tools and rejects non-HTTP navigation', () => {
  const input = [
    { jsonrpc: '2.0', id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25' } },
    { jsonrpc: '2.0', id: 2, method: 'tools/list' },
    { jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'open', arguments: { url: 'file:///etc/passwd' } } }
  ].map((message) => JSON.stringify(message)).join('\n') + '\n';
  const result = spawnSync(process.execPath, [serverPath], { input, encoding: 'utf8', timeout: 10_000 });
  assert.equal(result.status, 0, result.stderr);
  const responses = result.stdout.trim().split('\n').map((line) => JSON.parse(line));
  assert.equal(responses[0].result.serverInfo.name, 'beale-browser-use');
  assert.deepEqual(responses[1].result.tools.map((tool) => tool.name), [
    'list_tabs', 'open', 'navigate', 'observe', 'click', 'fill', 'capture', 'close'
  ]);
  assert.equal(responses[1].result.tools.find((tool) => tool.name === 'click').annotations['beale.io/tool'].confirmation, 'always');
  assert.equal(responses[2].result.isError, true);
  assert.match(responses[2].result.content[0].text, /Only HTTP\(S\)/);
});

const executable = process.env.BEALE_BROWSER_TEST_EXECUTABLE;

test('browser-use navigates and interacts through a real BiDi browser', { skip: !executable, timeout: 90_000 }, async () => {
  const site = createServer((_request, response) => {
    response.writeHead(200, { 'Content-Type': 'text/html' });
    response.end('<!doctype html><title>Example Page</title><h1>Browser fixture</h1><input aria-label="Search"><button onclick="document.querySelector(\'h1\').textContent = document.querySelector(\'input\').value">Apply</button><input type="password" aria-label="Secret">');
  });
  await new Promise((resolve) => site.listen(0, '127.0.0.1', resolve));
  const child = spawn(process.execPath, [serverPath], { env: { ...process.env, BEALE_BROWSER_EXECUTABLE_PATH: executable }, stdio: ['pipe', 'pipe', 'pipe'] });
  let stdout = '';
  let stderr = '';
  let nextId = 0;
  const pending = new Map();
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    stdout += chunk;
    while (stdout.includes('\n')) {
      const index = stdout.indexOf('\n');
      const line = stdout.slice(0, index);
      stdout = stdout.slice(index + 1);
      const response = JSON.parse(line);
      pending.get(response.id)?.(response);
      pending.delete(response.id);
    }
  });
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  const call = (name, args) => new Promise((resolve, reject) => {
    const id = ++nextId;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`Timed out waiting for ${name}: ${stderr}`)); }, 45_000);
    pending.set(id, (response) => { clearTimeout(timer); resolve(response.result); });
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method: 'tools/call', params: { name, arguments: args } })}\n`);
  });
  try {
    const address = site.address();
    const openResult = await call('open', { url: `http://127.0.0.1:${address.port}/` });
    assert.equal(openResult.isError, undefined, openResult.content[0].text);
    const opened = JSON.parse(openResult.content[0].text);
    const before = JSON.parse((await call('observe', { tabId: opened.tabId })).content[0].text);
    assert.match(before.text, /Browser fixture/);
    assert.equal(before.elements.some((element) => element.text === 'Secret'), false);
    const field = before.elements.find((element) => element.text === 'Search');
    assert.ok(field);
    assert.equal((await call('fill', { tabId: opened.tabId, observationId: field.observationId, text: 'Updated' })).isError, undefined);
    const afterFill = JSON.parse((await call('observe', { tabId: opened.tabId })).content[0].text);
    const button = afterFill.elements.find((element) => element.text === 'Apply');
    assert.ok(button);
    assert.equal((await call('click', { tabId: opened.tabId, observationId: button.observationId })).isError, undefined);
    const afterClick = JSON.parse((await call('observe', { tabId: opened.tabId })).content[0].text);
    assert.match(afterClick.text, /Updated/);
    assert.equal((await call('capture', { tabId: opened.tabId })).content[0].mimeType, 'image/png');
    assert.equal((await call('close', { tabId: opened.tabId })).isError, undefined);
  } finally {
    child.kill();
    site.close();
  }
});
