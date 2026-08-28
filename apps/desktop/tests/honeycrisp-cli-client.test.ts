import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  BEALE_APP_SERVER_CAPABILITIES,
  BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
  BEALE_APP_SERVER_CONTROL_VERSION,
  HONEYCRISP_CONTRACT_VERSION,
  HONEYCRISP_PROTOCOL_CAPABILITIES
} from 'honeycrisp/protocol';
import {
  decodeHoneycrispProtocolEnvelope,
  decodeHoneycrispMemorySummary,
  getHoneycrispProtocolDescriptor,
  invokeHoneycrispCliProtocol,
  invokeHoneycrispCliProtocolAsync,
  listHoneycrispSessionSummariesForWorkspacesAsync
} from '../src/main/honeycrispCliClient';

const createdDirectories: string[] = [];
const compatibleDescriptor = {
  protocol: 'honeycrisp',
  protocolVersion: 1,
  contractVersion: HONEYCRISP_CONTRACT_VERSION,
  runtime: { name: 'honeycrisp', version: '0.1.0', buildId: 'fixture-build', nodeVersion: process.version },
  schemas: { protocol: 1, session: 1, memorySummary: 11, finding: 4, campaignGraph: 4, goalSuggestions: 1 },
  capabilities: [...HONEYCRISP_PROTOCOL_CAPABILITIES],
  operations: ['protocol.describe'],
  transports: {
    appServer: { path: '/v1/operations', authentication: 'operator-bearer', framing: 'json', errors: 'http-problem' },
    websocket: { path: '/v1/session', authentication: 'bearer', framing: 'json-message', errors: 'protocol-error-message', correlation: 'request-id', capabilities: ['session.events', 'session.controls'] }
  }
};

afterEach(() => {
  delete process.env.BEALE_HONEYCRISP_COMMAND;
  delete process.env.BEALE_HONEYCRISP_ARGS_JSON;
  delete process.env.BEALE_HONEYCRISP_PROTOCOL_COMMAND;
  delete process.env.BEALE_HONEYCRISP_PROTOCOL_ARGS_JSON;
  delete process.env.BEALE_APP_SERVER_STATE_FILE;
  for (const directory of createdDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

describe('Honeycrisp app-server protocol client', () => {
  it('rejects unversioned and unsupported envelopes', () => {
    expect(() => decodeHoneycrispProtocolEnvelope('{}')).toThrow(/Invalid or unsupported/);
    expect(() => decodeHoneycrispProtocolEnvelope(JSON.stringify({
      protocol: 'honeycrisp',
      protocolVersion: 2,
      operation: 'protocol.describe',
      ok: true,
      result: {}
    }))).toThrow(/Invalid or unsupported/);
  });

  it('discovers the Honeycrisp protocol through the CLI boundary', () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-honeycrisp-protocol-'));
    createdDirectories.push(directory);
    const fixture = join(directory, 'protocol-fixture.mjs');
    writeFileSync(fixture, [
      '#!/usr/bin/env node',
      "const args = process.argv.slice(2);",
      "const requestId = args[args.indexOf('--request-id') + 1];",
      "if (args.slice(0, 3).join(' ') !== 'protocol describe --json' || !requestId) process.exit(2);",
      `console.log(JSON.stringify({ ...${JSON.stringify({
        protocol: 'honeycrisp',
        protocolVersion: 1,
        operation: 'protocol.describe',
        ok: true,
        result: compatibleDescriptor
      })}, requestId }));`
    ].join('\n'));
    chmodSync(fixture, 0o700);
    process.env.BEALE_HONEYCRISP_PROTOCOL_COMMAND = process.execPath;
    process.env.BEALE_HONEYCRISP_PROTOCOL_ARGS_JSON = JSON.stringify([fixture]);

    expect(getHoneycrispProtocolDescriptor()).toMatchObject({
      protocol: 'honeycrisp',
      protocolVersion: 1,
      operations: ['protocol.describe'],
      contractVersion: HONEYCRISP_CONTRACT_VERSION,
      transports: { websocket: { path: '/v1/session' } }
    });
  });

  it('rejects a CLI response correlated to a different request', () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-honeycrisp-protocol-'));
    createdDirectories.push(directory);
    const fixture = join(directory, 'protocol-fixture.mjs');
    writeFileSync(fixture, [
      '#!/usr/bin/env node',
      "console.log(JSON.stringify({ protocol: 'honeycrisp', protocolVersion: 1, operation: 'protocol.describe', requestId: 'wrong-request', ok: true, result: {} }));"
    ].join('\n'));
    chmodSync(fixture, 0o700);
    process.env.BEALE_HONEYCRISP_PROTOCOL_COMMAND = process.execPath;
    process.env.BEALE_HONEYCRISP_PROTOCOL_ARGS_JSON = JSON.stringify([fixture]);

    expect(() => invokeHoneycrispCliProtocol('protocol.describe', ['protocol', 'describe', '--json']))
      .toThrow(/request mismatch/);
  });

  it('reports protocol process failures without mistaking runtime warnings for the cause', () => {
    process.env.BEALE_HONEYCRISP_PROTOCOL_COMMAND = join(tmpdir(), 'missing-honeycrisp-protocol-command');

    expect(() => invokeHoneycrispCliProtocol('session.list', ['session', 'list', '--workspace-id', 'workspace_one', '--json']))
      .toThrow(/process error:.*ENOENT/);
  });

  it('suppresses Node runtime warnings on machine-readable protocol subprocesses', () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-honeycrisp-protocol-'));
    createdDirectories.push(directory);
    const fixture = join(directory, 'protocol-warning-fixture.mjs');
    writeFileSync(fixture, [
      '#!/usr/bin/env node',
      "const args = process.argv.slice(2);",
      "const requestId = args[args.indexOf('--request-id') + 1];",
      `if (args[0] === 'protocol' && args[1] === 'describe') { console.log(JSON.stringify({ protocol: 'honeycrisp', protocolVersion: 1, operation: 'protocol.describe', requestId, ok: true, result: ${JSON.stringify(compatibleDescriptor)} })); process.exit(0); }`,
      "if (process.env.NODE_NO_WARNINGS !== '1') process.stderr.write('ExperimentalWarning: protocol noise\\n');",
      "console.log(JSON.stringify({ protocol: 'honeycrisp', protocolVersion: 1, operation: 'session.list', requestId, ok: true, result: [] }));"
    ].join('\n'));
    chmodSync(fixture, 0o700);
    process.env.BEALE_HONEYCRISP_PROTOCOL_COMMAND = process.execPath;
    process.env.BEALE_HONEYCRISP_PROTOCOL_ARGS_JSON = JSON.stringify([fixture]);

    expect(invokeHoneycrispCliProtocol('session.list', ['session', 'list', '--workspace-id', 'workspace_one', '--json']).result)
      .toEqual([]);
  });

  it('retains complete JSON envelopes larger than the former two-million-character process cap', () => {
    const envelope = decodeHoneycrispProtocolEnvelope<{ text: string }>(JSON.stringify({
      protocol: 'honeycrisp',
      protocolVersion: 1,
      operation: 'session.get',
      requestId: 'large-response',
      ok: true,
      result: { text: 'v'.repeat(3 * 1024 * 1024) }
    }));
    expect(envelope.ok).toBe(true);
    if (!envelope.ok) throw new Error(envelope.error.message);
    expect(envelope.result.text).toHaveLength(3 * 1024 * 1024);
  });

  it('rejects incompatible runtime descriptors and malformed memory summary v9 payloads', () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-honeycrisp-incompatible-'));
    createdDirectories.push(directory);
    const fixture = join(directory, 'protocol-fixture.mjs');
    writeFileSync(fixture, [
      '#!/usr/bin/env node',
      "const args = process.argv.slice(2);",
      "const requestId = args[args.indexOf('--request-id') + 1];",
      `console.log(JSON.stringify({ protocol: 'honeycrisp', protocolVersion: 1, operation: 'protocol.describe', requestId, ok: true, result: ${JSON.stringify({ ...compatibleDescriptor, contractVersion: 1 })} }));`
    ].join('\n'));
    chmodSync(fixture, 0o700);
    process.env.BEALE_HONEYCRISP_PROTOCOL_COMMAND = process.execPath;
    process.env.BEALE_HONEYCRISP_PROTOCOL_ARGS_JSON = JSON.stringify([fixture]);
    expect(() => getHoneycrispProtocolDescriptor()).toThrow(new RegExp(`incompatible with Beale contract v${HONEYCRISP_CONTRACT_VERSION}`));
    expect(() => decodeHoneycrispMemorySummary({ nodes: [], edges: [], runbooks: [], leads: [], findings: [], campaign: {} })).toThrow(/memory summary v9/);
    expect(() => decodeHoneycrispMemorySummary({
      nodeCount: 0,
      edgeCount: 0,
      nodes: [],
      edges: [],
      runbooks: [],
      leads: [],
      findings: [],
      campaign: {
        nodes: [], edges: [], coverageGaps: [], contradictions: [], nextActions: [],
        momentum: { state: 'empty', reason: 'No campaign work.', supportingNodeIds: [] },
        counts: { findings: 0, coverageGaps: 0 },
        tracks: [{ id: 'unbounded-cast' }]
      }
    })).toThrow(/memory summary v9/);
  });

  it('validates bounded question, experiment, and observation summaries in campaign tracks', () => {
    const experiment = {
      id: 'experiment_one',
      investigationId: 'investigation_one',
      questionId: 'question_one',
      runbookId: null,
      title: 'Exercise resolver transition',
      status: 'succeeded',
      resultSummary: 'Observed the expected transition.',
      startedAt: '2026-08-26T10:00:00.000Z',
      completedAt: '2026-08-26T10:15:00.000Z',
      updatedAt: '2026-08-26T10:15:00.000Z',
      revision: 1
    };
    const question = {
      id: 'question_one',
      investigationId: 'investigation_one',
      text: 'Is the transition reachable?',
      status: 'answered',
      priority: 'high',
      answer: 'Yes.',
      updatedAt: '2026-08-26T10:15:00.000Z',
      revision: 2
    };
    const observation = {
      id: 'observation_one',
      investigationId: 'investigation_one',
      experimentId: 'experiment_one',
      kind: 'runtime',
      outcome: 'supports',
      summary: 'Observed the expected transition.',
      createdAt: '2026-08-26T10:14:00.000Z'
    };
    const track = {
      id: 'investigation_one',
      title: 'Resolver transitions',
      objective: 'Determine whether the transition is reachable.',
      status: 'active',
      stage: 'testing',
      source: 'runtime',
      sessionIds: ['session_one'],
      updatedAt: '2026-08-26T10:15:00.000Z',
      revision: 1,
      questions: [question],
      experiments: [experiment],
      observations: [observation],
      counts: { questions: 1, openQuestions: 0, experiments: 1, observations: 1, openNextActions: 0, memoryNodes: 0, evidenceRefs: 1, findings: 0, runbooks: 0, reports: 0 }
    };
    const summary = {
      nodeCount: 0,
      edgeCount: 0,
      nodes: [],
      edges: [],
      runbooks: [],
      leads: [],
      findings: [],
      campaign: {
        nodes: [], edges: [], coverageGaps: [], contradictions: [], nextActions: [],
        momentum: { state: 'testing', reason: 'An experiment completed.', supportingNodeIds: [] },
        counts: { leads: 0, findings: 0, coverageGaps: 0 },
        tracks: [track]
      }
    };

    expect(decodeHoneycrispMemorySummary(summary).campaign.tracks?.[0]).toMatchObject({
      questions: [question],
      experiments: [experiment],
      observations: [observation]
    });
    const { questions: _questions, ...questionlessTrack } = track;
    expect(() => decodeHoneycrispMemorySummary({
      ...summary,
      campaign: { ...summary.campaign, tracks: [questionlessTrack] }
    })).toThrow(/memory summary v9/);
    const { experiments: _experiments, ...countOnlyTrack } = track;
    expect(() => decodeHoneycrispMemorySummary({
      ...summary,
      campaign: { ...summary.campaign, tracks: [countOnlyTrack] }
    })).toThrow(/memory summary v9/);
    const { observations: _observations, ...observationlessTrack } = track;
    expect(() => decodeHoneycrispMemorySummary({
      ...summary,
      campaign: { ...summary.campaign, tracks: [observationlessTrack] }
    })).toThrow(/memory summary v9/);
    expect(() => decodeHoneycrispMemorySummary({
      ...summary,
      campaign: { ...summary.campaign, tracks: [{ ...track, experiments: [{ ...experiment, status: 'unknown' }] }] }
    })).toThrow(/memory summary v9/);
  });

  it('batches multiple workspace summary catalogs into one app-server operation', async () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-honeycrisp-protocol-'));
    createdDirectories.push(directory);
    let receivedBody: Record<string, unknown> | null = null;
    const server = createServer((request, response) => {
      if (request.url === '/health') {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({
          ok: true,
          controlVersion: BEALE_APP_SERVER_CONTROL_VERSION,
          contractTimestamp: BEALE_APP_SERVER_CONTRACT_TIMESTAMP,
          capabilities: BEALE_APP_SERVER_CAPABILITIES
        }));
        return;
      }
      let body = '';
      request.on('data', (chunk: Buffer) => { body += chunk.toString('utf8'); });
      request.on('end', () => {
        receivedBody = JSON.parse(body) as Record<string, unknown>;
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ controlVersion: BEALE_APP_SERVER_CONTROL_VERSION, result: [] }));
      });
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    try {
      const address = server.address();
      if (!address || typeof address === 'string') throw new Error('Expected a loopback test server.');
      const url = `http://127.0.0.1:${address.port}`;
      const stateFile = join(directory, 'app-server.json');
      process.env.BEALE_APP_SERVER_STATE_FILE = stateFile;
      writeFileSync(stateFile, JSON.stringify({
        version: 1,
        hostMode: 'tray',
        pid: process.pid,
        host: '127.0.0.1',
        port: address.port,
        localUrl: url,
        url,
        operatorToken: 'test-operator-token',
        startedAt: new Date().toISOString()
      }), 'utf8');

      await expect(listHoneycrispSessionSummariesForWorkspacesAsync(
        ['workspace_one', 'workspace_two', 'workspace_one'],
        { databasePath: join(directory, 'memory.sqlite'), artifactDirectoryPath: join(directory, 'artifacts') }
      )).resolves.toEqual([]);
      expect(receivedBody).toMatchObject({
        operation: 'session.list_summaries',
        args: [
          'session', 'list-summaries',
          '--workspace-id', 'workspace_one',
          '--workspace-id', 'workspace_two',
          '--limit', '200', '--json'
        ]
      });
    } finally {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
});
