import assert from 'node:assert/strict';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { readPersistedRemoteAccessLaunchOptions } from '../dist/remoteAccessConfig.js';

test('restores an enabled persisted MagicDNS endpoint for a standalone tray launch', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-remote-access-config-'));
  try {
    const path = join(directory, 'remote-access.json');
    writeFileSync(path, JSON.stringify({
      version: 1,
      enabled: true,
      magicDnsName: 'Beale-PC.Tailnet.ts.net.',
      localPort: 47_173,
      httpsPort: 47_174,
    }));

    assert.deepEqual(readPersistedRemoteAccessLaunchOptions(path), {
      host: '127.0.0.1',
      port: 47_173,
      publicUrl: 'https://beale-pc.tailnet.ts.net:47174',
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('does not advertise persisted endpoints that are disabled or malformed', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-remote-access-config-'));
  try {
    const path = join(directory, 'remote-access.json');
    writeFileSync(path, JSON.stringify({
      version: 1,
      enabled: false,
      magicDnsName: 'beale-pc.tailnet.ts.net',
      localPort: 47_173,
      httpsPort: 47_174,
    }));
    assert.equal(readPersistedRemoteAccessLaunchOptions(path), null);

    writeFileSync(path, JSON.stringify({
      version: 1,
      enabled: true,
      magicDnsName: 'example.com',
      localPort: 47_173,
      httpsPort: 47_174,
    }));
    assert.equal(readPersistedRemoteAccessLaunchOptions(path), null);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
