#!/usr/bin/env node

import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { join } from 'node:path';

const SERVER_NAME = 'beale-codex';
const SERVER_VERSION = '0.1.0';
const REQUEST_TIMEOUT_MS = 120_000;

const TOOLS = [
  tool('beale_list_workspaces', 'List path-free Beale workspaces known to the running app-server.', {}, readOnly()),
  tool('beale_list_sessions', 'List canonical research sessions in one Beale workspace.', {
    workspaceId: stringField('Beale workspace id.'),
    limit: { type: 'number', minimum: 1, maximum: 200 }
  }, readOnly(), ['workspaceId']),
  tool('beale_get_session', 'Read a bounded canonical session update, including transcript and status.', {
    workspaceId: stringField('Beale workspace id.'),
    sessionId: stringField('Beale session id.'),
    afterEventId: stringField('Optional event cursor.'),
    tail: { type: 'boolean', default: true },
    limit: { type: 'number', minimum: 1, maximum: 1000 },
    maxBytes: { type: 'number', minimum: 1024, maximum: 4000000 }
  }, readOnly(), ['workspaceId', 'sessionId']),
  tool('beale_start_session', 'Start a Beale-hosted research agent in a registered workspace.', {
    workspaceId: stringField('Beale workspace id.'),
    promptMarkdown: stringField('Complete research instruction.'),
    provider: stringField('Optional Beale provider id.'),
    model: stringField('Optional provider model id.'),
    reasoningEffort: stringField('Optional reasoning effort.'),
    fastMode: { type: 'boolean' },
    workflowId: stringField('Optional research-profile workflow id.'),
    goalObjective: stringField('Optional persistent goal objective.'),
    shellSafetyMode: { type: 'string', enum: ['manual_approval', 'auto_review', 'danger'], default: 'auto_review' }
  }, mutating({ openWorldHint: true }), ['workspaceId', 'promptMarkdown']),
  tool('beale_steer_session', 'Send a follow-up instruction to an active Beale research session.', {
    sessionId: stringField('Active Beale session id.'),
    instruction: stringField('Follow-up instruction for the running agent.')
  }, mutating(), ['sessionId', 'instruction']),
  tool('beale_pause_session', 'Pause or resume an active Beale research session.', {
    sessionId: stringField('Active Beale session id.'),
    paused: { type: 'boolean' }
  }, mutating(), ['sessionId', 'paused']),
  tool('beale_stop_session', 'Stop an active Beale research session.', {
    sessionId: stringField('Active Beale session id.')
  }, mutating({ destructiveHint: true }), ['sessionId']),
  tool('beale_continue_session', 'Continue a terminal Beale session with an appended instruction and retained research history.', {
    workspaceId: stringField('Beale workspace id.'),
    sessionId: stringField('Terminal Beale session id.'),
    instruction: stringField('Continuation instruction.')
  }, mutating({ openWorldHint: true }), ['workspaceId', 'sessionId', 'instruction']),
  tool('beale_list_channels', 'List durable collaboration channels in a Beale workspace.', {
    workspaceId: stringField('Beale workspace id.'),
    limit: { type: 'number', minimum: 1, maximum: 1000 },
    archived: { type: 'boolean' }
  }, readOnly(), ['workspaceId']),
  tool('beale_get_channel', 'Read one durable Beale collaboration channel and its recent messages.', {
    workspaceId: stringField('Beale workspace id.'),
    channel: stringField('Channel id or slug.'),
    messageLimit: { type: 'number', minimum: 1, maximum: 1000 }
  }, readOnly(), ['workspaceId', 'channel']),
  tool('beale_post_channel_message', 'Post a Codex-authored message to a durable Beale collaboration channel.', {
    workspaceId: stringField('Beale workspace id.'),
    channel: stringField('Channel id or slug.'),
    contentMarkdown: stringField('Message content.'),
    kind: { type: 'string', enum: ['message', 'status', 'request', 'result'] },
    evidenceRefs: { type: 'array', items: { type: 'string' } }
  }, mutating(), ['workspaceId', 'channel', 'contentMarkdown']),
  tool('beale_list_research_tools', 'List the exact in-Beale durable research tools enabled by a workspace profile.', {
    workspaceId: stringField('Beale workspace id.'),
    sessionId: stringField('Optional Beale session association for new records.'),
    investigationId: stringField('Optional campaign track id; includes track-scoped tools when present.'),
    objective: stringField('Optional bounded campaign objective for resource relevance.')
  }, readOnly(), ['workspaceId']),
  tool('beale_read_research', 'Invoke one read-only in-Beale research tool against canonical workspace state.', researchCallProperties(), readOnly(), ['workspaceId', 'toolName', 'toolInput']),
  tool('beale_write_research', 'Invoke one revisioned or side-effecting in-Beale research tool against canonical workspace state. Codex approval applies before this MCP call.', researchCallProperties(), mutating({ openWorldHint: true }), ['workspaceId', 'toolName', 'toolInput'])
];

let inputBuffer = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  inputBuffer += chunk;
  drainMessages();
});
process.stdin.on('error', (error) => console.error(errorMessage(error)));

function drainMessages() {
  for (;;) {
    const newline = inputBuffer.indexOf('\n');
    if (newline < 0) return;
    const line = inputBuffer.slice(0, newline).replace(/\r$/u, '').trim();
    inputBuffer = inputBuffer.slice(newline + 1);
    if (line) void handleMessage(line);
  }
}

async function handleMessage(line) {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    sendError(null, -32700, 'Invalid JSON-RPC payload.');
    return;
  }
  if (message.method?.startsWith('notifications/')) return;
  try {
    if (message.method === 'initialize') {
      sendResult(message.id, {
        protocolVersion: message.params?.protocolVersion ?? '2024-11-05',
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: SERVER_NAME, version: SERVER_VERSION },
        instructions: 'Use Beale workspace IDs. List research tools before invoking profile-scoped reads or writes.'
      });
      return;
    }
    if (message.method === 'ping') {
      sendResult(message.id, {});
      return;
    }
    if (message.method === 'tools/list') {
      sendResult(message.id, { tools: TOOLS });
      return;
    }
    if (message.method === 'tools/call') {
      const result = await callTool(message.params?.name, record(message.params?.arguments));
      sendResult(message.id, mcpResult(result));
      return;
    }
    sendError(message.id ?? null, -32601, `Method not found: ${String(message.method)}`);
  } catch (error) {
    sendResult(message.id ?? null, {
      content: [{ type: 'text', text: errorMessage(error) }],
      isError: true
    });
  }
}

async function callTool(name, args) {
  switch (name) {
    case 'beale_list_workspaces': return request('/v1/workspaces');
    case 'beale_list_sessions': return request(`/v1/workspaces/${part(required(args, 'workspaceId'))}/sessions${query({ limit: args.limit })}`);
    case 'beale_get_session': return request(`/v1/workspaces/${part(required(args, 'workspaceId'))}/sessions/${part(required(args, 'sessionId'))}/update${query({
      afterEventId: args.afterEventId,
      tail: args.tail ?? true,
      limit: args.limit ?? 200,
      maxBytes: args.maxBytes ?? 1000000
    })}`);
    case 'beale_start_session': return request('/v1/sessions', {
      method: 'POST',
      body: {
        launchVersion: 2,
        launch: {
          workspaceId: required(args, 'workspaceId'),
          promptMarkdown: required(args, 'promptMarkdown'),
          shellSafetyMode: args.shellSafetyMode ?? 'auto_review',
          ...(args.workflowId ? { workflowId: args.workflowId } : {}),
          ...(args.goalObjective ? { goal: { objective: args.goalObjective } } : {}),
          ...(args.provider || args.model || args.reasoningEffort || args.fastMode === true ? {
            provider: {
              ...(args.provider ? { id: args.provider } : {}),
              ...(args.model ? { model: args.model } : {}),
              ...(args.reasoningEffort ? { reasoningEffort: args.reasoningEffort } : {}),
              ...(args.fastMode === true ? { fastMode: true } : {})
            }
          } : {})
        }
      }
    });
    case 'beale_steer_session': return controlSession(required(args, 'sessionId'), {
      type: 'steer', instruction: required(args, 'instruction')
    });
    case 'beale_pause_session': {
      if (typeof args.paused !== 'boolean') throw new Error('paused must be a boolean.');
      return controlSession(required(args, 'sessionId'), { type: args.paused ? 'pause' : 'resume' });
    }
    case 'beale_stop_session': return request(`/v1/sessions/${part(required(args, 'sessionId'))}`, { method: 'DELETE' });
    case 'beale_continue_session': return request(`/v1/sessions/${part(required(args, 'sessionId'))}/continuations`, {
      method: 'POST',
      body: {
        workspaceId: required(args, 'workspaceId'),
        instruction: required(args, 'instruction')
      }
    });
    case 'beale_list_channels': return request(`/v1/workspaces/${part(required(args, 'workspaceId'))}/channels${query({
      limit: args.limit ?? 200,
      archived: args.archived
    })}`);
    case 'beale_get_channel': return request(`/v1/workspaces/${part(required(args, 'workspaceId'))}/channels/${part(required(args, 'channel'))}${query({
      messageLimit: args.messageLimit ?? 500
    })}`);
    case 'beale_post_channel_message': return request(`/v1/workspaces/${part(required(args, 'workspaceId'))}/channels/${part(required(args, 'channel'))}`, {
      method: 'POST',
      body: {
        contentMarkdown: required(args, 'contentMarkdown'),
        ...(args.kind ? { kind: args.kind } : {}),
        ...(Array.isArray(args.evidenceRefs) ? { evidenceRefs: args.evidenceRefs } : {})
      }
    });
    case 'beale_list_research_tools': return researchOperation('research.tools.list', args);
    case 'beale_read_research': return researchOperation('research.tools.read', args);
    case 'beale_write_research': return researchOperation('research.tools.mutate', args);
    default: throw new Error(`Unknown Beale Codex tool: ${String(name)}`);
  }
}

function controlSession(sessionId, body) {
  return request(`/v1/sessions/${part(sessionId)}/control`, { method: 'POST', body });
}

function researchOperation(operation, args) {
  const toolName = operation === 'research.tools.list' ? null : required(args, 'toolName');
  const toolInput = operation === 'research.tools.list' ? null : recordOrNull(args.toolInput);
  if (operation !== 'research.tools.list' && !toolInput) throw new Error('toolInput must be an object.');
  return request('/v1/operations', {
    method: 'POST',
    body: {
      operation,
      input: {
        workspaceId: required(args, 'workspaceId'),
        ...(args.sessionId ? { sessionId: args.sessionId } : {}),
        ...(args.investigationId ? { investigationId: args.investigationId } : {}),
        ...(args.objective ? { objective: args.objective } : {}),
        ...(toolName ? { toolName } : {}),
        ...(toolInput ? { toolInput } : {}),
        modelAuthor: {
          provider: 'openai-codex',
          model: process.env.CODEX_MODEL?.trim() || 'codex'
        }
      }
    }
  });
}

async function request(pathname, options = {}) {
  const discovery = await appServerDiscovery();
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    const response = await fetch(new URL(pathname, discovery.url), {
      method: options.method ?? 'GET',
      headers: {
        authorization: `Bearer ${discovery.token}`,
        ...(options.body === undefined ? {} : { 'content-type': 'application/json' })
      },
      ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
      signal: controller.signal
    });
    const text = await response.text();
    let payload;
    try {
      payload = text ? JSON.parse(text) : null;
    } catch {
      payload = { error: text || `HTTP ${response.status}` };
    }
    if (!response.ok) {
      const detail = payload?.error?.message ?? payload?.error ?? `HTTP ${response.status}`;
      throw new Error(`Beale app-server request failed: ${String(detail)}`);
    }
    return payload;
  } catch (error) {
    if (error?.name === 'AbortError') throw new Error('Beale app-server request timed out.');
    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

async function appServerDiscovery() {
  const configuredUrl = process.env.BEALE_APP_SERVER_URL?.trim();
  const configuredToken = process.env.BEALE_APP_SERVER_OPERATOR_TOKEN?.trim();
  if (configuredUrl || configuredToken) {
    if (!configuredUrl || !configuredToken) {
      throw new Error('BEALE_APP_SERVER_URL and BEALE_APP_SERVER_OPERATOR_TOKEN must be configured together.');
    }
    return { url: validatedUrl(configuredUrl), token: configuredToken };
  }
  const statePath = process.env.BEALE_APP_SERVER_STATE_FILE?.trim() || join(homedir(), '.beale', 'app-server.json');
  let parsed;
  try {
    parsed = JSON.parse(await readFile(statePath, 'utf8'));
  } catch {
    throw new Error(`Beale app-server discovery is unavailable at ${statePath}. Start Beale or app-server first.`);
  }
  const localUrl = typeof parsed.localUrl === 'string' ? parsed.localUrl.trim() : '';
  const token = typeof parsed.operatorToken === 'string' ? parsed.operatorToken.trim() : '';
  if (!localUrl || !token) throw new Error('Beale app-server discovery record is incomplete.');
  return { url: validatedUrl(localUrl), token };
}

function validatedUrl(value) {
  const url = new URL(value);
  const loopback = url.hostname === '127.0.0.1' || url.hostname === 'localhost' || url.hostname === '[::1]' || url.hostname === '::1';
  if (!loopback && process.env.BEALE_APP_SERVER_ALLOW_REMOTE !== '1') {
    throw new Error('Beale Codex uses the app-server loopback endpoint unless BEALE_APP_SERVER_ALLOW_REMOTE=1 is explicitly configured.');
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('Beale app-server URL must use HTTP or HTTPS.');
  return url;
}

function mcpResult(value) {
  return {
    content: [{ type: 'text', text: JSON.stringify(value, null, 2) }],
    structuredContent: value && typeof value === 'object' && !Array.isArray(value) ? value : { value },
    isError: false
  };
}

function tool(name, description, properties, annotations, requiredFields = []) {
  return {
    name,
    description,
    inputSchema: {
      type: 'object',
      properties,
      ...(requiredFields.length ? { required: requiredFields } : {}),
      additionalProperties: false
    },
    annotations
  };
}

function researchCallProperties() {
  return {
    workspaceId: stringField('Beale workspace id.'),
    sessionId: stringField('Optional Beale session association for authored records.'),
    investigationId: stringField('Optional active campaign track id.'),
    objective: stringField('Optional bounded campaign objective.'),
    toolName: stringField('Canonical research tool name returned by beale_list_research_tools.'),
    toolInput: { type: 'object', additionalProperties: true }
  };
}

function stringField(description) {
  return { type: 'string', minLength: 1, description };
}

function readOnly() {
  return { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false };
}

function mutating(overrides = {}) {
  return { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false, ...overrides };
}

function query(values) {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value !== undefined && value !== null && value !== '') params.set(key, String(value));
  }
  const rendered = params.toString();
  return rendered ? `?${rendered}` : '';
}

function part(value) {
  return encodeURIComponent(String(value));
}

function required(args, name) {
  const value = args[name];
  if (typeof value !== 'string' || !value.trim()) throw new Error(`${name} is required.`);
  return value.trim();
}

function record(value) {
  return recordOrNull(value) ?? {};
}

function recordOrNull(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : null;
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

function sendResult(id, result) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id, result })}\n`);
}

function sendError(id, code, message) {
  process.stdout.write(`${JSON.stringify({ jsonrpc: '2.0', id, error: { code, message } })}\n`);
}
