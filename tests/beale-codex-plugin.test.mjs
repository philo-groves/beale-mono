import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import test from 'node:test';

const pluginServer = resolve('integrations/beale-codex/mcp/server.mjs');

test('Beale Codex MCP advertises approval-aware research and session tools', async (t) => {
  const mock = await createMockAppServer();
  t.after(() => mock.close());
  const client = await createMcpClient(mock.stateFile);
  t.after(() => client.close());

  const initialized = await client.request('initialize', { protocolVersion: '2025-06-18' });
  assert.equal(initialized.serverInfo.name, 'beale-codex');

  const listed = await client.request('tools/list', {});
  const tools = new Map(listed.tools.map((tool) => [tool.name, tool]));
  assert.equal(tools.get('beale_list_workspaces').annotations.readOnlyHint, true);
  assert.equal(tools.get('beale_read_research').annotations.readOnlyHint, true);
  assert.equal(tools.get('beale_write_research').annotations.readOnlyHint, false);
  assert.equal(tools.get('beale_stop_session').annotations.destructiveHint, true);
  assert.ok(tools.has('beale_steer_session'));
  assert.ok(tools.has('beale_continue_session'));
});

test('Beale Codex MCP keeps discovery credentials internal and routes canonical operations', async (t) => {
  const mock = await createMockAppServer();
  t.after(() => mock.close());
  const client = await createMcpClient(mock.stateFile);
  t.after(() => client.close());

  const workspaces = await client.call('beale_list_workspaces', {});
  assert.equal(workspaces.structuredContent.workspaces[0].workspaceId, 'workspace-example');
  assert.doesNotMatch(JSON.stringify(workspaces), /operator-secret/u);

  await client.call('beale_read_research', {
    workspaceId: 'workspace-example',
    toolName: 'history.search',
    toolInput: { query: 'parser' }
  });
  await client.call('beale_write_research', {
    workspaceId: 'workspace-example',
    sessionId: 'session-example',
    toolName: 'lead.create',
    toolInput: { title: 'Candidate', classification: 'security.vulnerability', rating: 'medium' }
  });
  await client.call('beale_steer_session', {
    sessionId: 'session-example',
    instruction: 'Verify the parser boundary.'
  });

  assert.equal(mock.requests.length, 4);
  assert.ok(mock.requests.every((request) => request.authorization === 'Bearer operator-secret'));
  assert.deepEqual(mock.requests[1].body, {
    operation: 'research.tools.read',
    input: {
      workspaceId: 'workspace-example',
      toolName: 'history.search',
      toolInput: { query: 'parser' },
      modelAuthor: { provider: 'openai-codex', model: 'codex' }
    }
  });
  assert.equal(mock.requests[2].body.operation, 'research.tools.mutate');
  assert.equal(mock.requests[2].body.input.sessionId, 'session-example');
  assert.deepEqual(mock.requests[3].body, { type: 'steer', instruction: 'Verify the parser boundary.' });
});

async function createMockAppServer() {
  const directory = await mkdtemp(join(tmpdir(), 'beale-codex-plugin-'));
  const stateFile = join(directory, 'app-server.json');
  const requests = [];
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const bodyText = Buffer.concat(chunks).toString('utf8');
    const body = bodyText ? JSON.parse(bodyText) : null;
    requests.push({
      method: request.method,
      url: request.url,
      authorization: request.headers.authorization,
      body
    });
    response.setHeader('content-type', 'application/json');
    if (request.url === '/v1/workspaces') {
      response.end(JSON.stringify({ workspaces: [{ workspaceId: 'workspace-example', name: 'Example' }] }));
      return;
    }
    response.statusCode = request.url?.endsWith('/control') ? 202 : 200;
    response.end(JSON.stringify({ ok: true, echo: body }));
  });
  await new Promise((resolvePromise) => server.listen(0, '127.0.0.1', resolvePromise));
  const address = server.address();
  const localUrl = `http://127.0.0.1:${address.port}`;
  await writeFile(stateFile, JSON.stringify({ localUrl, operatorToken: 'operator-secret' }), 'utf8');
  return {
    stateFile,
    requests,
    close: () => new Promise((resolvePromise, reject) => server.close((error) => error ? reject(error) : resolvePromise()))
  };
}

async function createMcpClient(stateFile) {
  const child = spawn(process.execPath, [pluginServer], {
    env: { ...process.env, BEALE_APP_SERVER_STATE_FILE: stateFile, CODEX_MODEL: '' },
    stdio: ['pipe', 'pipe', 'pipe']
  });
  const lines = createInterface({ input: child.stdout });
  const pending = [];
  let nextId = 1;
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  lines.on('line', (line) => {
    const waiter = pending.shift();
    if (!waiter) return;
    try {
      const message = JSON.parse(line);
      if (message.error) waiter.reject(new Error(message.error.message));
      else waiter.resolve(message.result);
    } catch (error) {
      waiter.reject(error);
    }
  });
  child.once('exit', (code) => {
    while (pending.length) pending.shift().reject(new Error(`MCP server exited ${code}: ${stderr}`));
  });
  return {
    request(method, params) {
      return new Promise((resolvePromise, reject) => {
        const id = nextId++;
        pending.push({ resolve: resolvePromise, reject });
        child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
      });
    },
    async call(name, args) {
      return this.request('tools/call', { name, arguments: args });
    },
    close() {
      lines.close();
      child.kill();
    }
  };
}
