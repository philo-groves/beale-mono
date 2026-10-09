import assert from 'node:assert/strict';
import test from 'node:test';
import { normalizeTailnetOrigin, verifySameTailnet } from '../dist/tailnetRemote.js';

const status = {
  BackendState: 'Running',
  Self: { DNSName: 'source.example-tailnet.ts.net.' },
  Peer: { peer: { DNSName: 'worker.example-tailnet.ts.net.', Online: true, TailscaleIPs: ['192.0.2.10'] } },
};
const runner = { async run(_command, args) { assert.deepEqual(args, ['status', '--json']); return JSON.stringify(status); } };

test('remote app-server endpoints require HTTPS MagicDNS and a matching online tailnet peer', async () => {
  const origin = normalizeTailnetOrigin('https://worker.example-tailnet.ts.net:47174');
  await verifySameTailnet(origin, runner, async () => ['192.0.2.10']);
  await assert.rejects(verifySameTailnet(origin, runner, async () => ['203.0.113.10']), /Tailscale peer address/);
  await assert.rejects(verifySameTailnet('https://worker.other-tailnet.ts.net:47174', runner, async () => ['192.0.2.10']), /outside this Tailscale network/);
  assert.throws(() => normalizeTailnetOrigin('http://worker.example-tailnet.ts.net:47174'), /HTTPS/);
  assert.throws(() => normalizeTailnetOrigin('https://worker.example-tailnet.ts.net:47174/path'), /HTTPS/);
});
