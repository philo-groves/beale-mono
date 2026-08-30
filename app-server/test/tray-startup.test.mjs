import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

test('uses the recoverable discovery lock as the sole tray startup gate', () => {
  const source = readFileSync(new URL('../src/trayMain.ts', import.meta.url), 'utf8');

  assert.match(source, /acquireDiscoveryLock\(stateFile, process\.pid\)/u);
  assert.doesNotMatch(source, /requestSingleInstanceLock|second-instance/u);
  assert.ok(source.indexOf('acquireDiscoveryLock(stateFile, process.pid)') < source.indexOf('startAppServer(serverOptions)'));
});

test('can attach tray controls to a live headless host without taking discovery ownership', () => {
  const source = readFileSync(new URL('../src/trayMain.ts', import.meta.url), 'utf8');

  assert.match(source, /const ATTACH_EXISTING_FLAG = '--attach-existing'/u);
  assert.match(source, /existing\.hostMode !== 'tray' && isProcessAlive\(existing\.pid\)/u);
  assert.match(source, /acquireDiscoveryLock\(trayControllerLockFile, process\.pid\)/u);
  assert.match(source, /decodeBealeAppServerSessionCatalog/u);
  assert.match(source, /Restart Beale App Server/u);
  assert.ok(
    source.indexOf("existing.hostMode !== 'tray'")
      < source.indexOf('acquireDiscoveryLock(stateFile, process.pid)')
  );
});

test('isolates the tray host Chromium profile from Beale Desktop', () => {
  const source = readFileSync(new URL('../src/trayMain.ts', import.meta.url), 'utf8');
  const userDataSetup = "app.setPath('userData', join(app.getPath('appData'), 'beale-app-server'))";

  assert.match(source, /app\.setPath\('userData', join\(app\.getPath\('appData'\), 'beale-app-server'\)\)/u);
  assert.ok(source.indexOf(userDataSetup) < source.indexOf('app.whenReady()'));
});

test('prevents macOS from suspending the tray-resident control plane', () => {
  const source = readFileSync(new URL('../src/trayMain.ts', import.meta.url), 'utf8');

  assert.match(source, /powerSaveBlocker\.start\('prevent-app-suspension'\)/u);
  assert.match(source, /powerSaveBlocker\.stop\(appSuspensionBlockerId\)/u);
  assert.ok(
    source.indexOf("powerSaveBlocker.start('prevent-app-suspension')")
      > source.indexOf('startAppServer(serverOptions)')
  );
});
