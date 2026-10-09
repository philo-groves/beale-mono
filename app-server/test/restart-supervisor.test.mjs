import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { restartSupervisedAppServer } from '../dist/restartSupervisor.js';

async function waitFor(check, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const result = check();
    if (result) return result;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error('Timed out waiting for the synthetic app-server.');
}

test('restart supervisor replaces only the process that owns the discovery lock', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-restart-supervisor-'));
  const discoveryFile = join(directory, 'app-server.json');
  const lockFile = `${discoveryFile}.lock`;
  const script = join(directory, 'synthetic-server.cjs');
  writeFileSync(script, `
    const { writeFileSync } = require('node:fs');
    const stateFile = ${JSON.stringify(discoveryFile)};
    const lockFile = ${JSON.stringify(lockFile)};
    writeFileSync(lockFile, JSON.stringify({ pid: process.pid }));
    writeFileSync(stateFile, JSON.stringify({ version: 1, contractTimestamp: null, hostMode: 'headless', pid: process.pid,
      host: '127.0.0.1', port: 47173, localUrl: 'http://127.0.0.1:47173', url: 'http://127.0.0.1:47173',
      operatorToken: 'synthetic-operator-token-00000000', startedAt: new Date().toISOString() }));
    setInterval(() => {}, 1_000);
  `);
  const first = spawn(process.execPath, [script], { stdio: 'ignore' });
  const originalFetch = globalThis.fetch;
  let replacementPid;
  try {
    await waitFor(() => {
      try { return JSON.parse(readFileSync(discoveryFile, 'utf8')).pid === first.pid; }
      catch { return false; }
    });
    globalThis.fetch = async () => new Response('{"ok":true}', { status: 200 });
    const config = {
      discoveryFile, command: process.execPath, args: [script], hostMode: 'headless',
      operatorToken: 'synthetic-operator-token-00000000', port: 0
    };

    writeFileSync(lockFile, JSON.stringify({ pid: process.pid }));
    await assert.rejects(restartSupervisedAppServer(config), /does not own its discovery lock/u);
    assert.equal(JSON.parse(readFileSync(discoveryFile, 'utf8')).pid, first.pid);

    writeFileSync(lockFile, JSON.stringify({ pid: first.pid }));
    await restartSupervisedAppServer(config);
    replacementPid = JSON.parse(readFileSync(discoveryFile, 'utf8')).pid;
    assert.notEqual(replacementPid, first.pid);
  } finally {
    globalThis.fetch = originalFetch;
    if (replacementPid) process.kill(replacementPid, 'SIGTERM');
    if (first.exitCode === null) first.kill('SIGTERM');
    rmSync(directory, { recursive: true, force: true });
  }
});
