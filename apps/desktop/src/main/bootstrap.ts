import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { app } from 'electron';

const APP_SERVER_TRAY_FLAG = '--beale-app-server-tray';

if (process.argv.includes(APP_SERVER_TRAY_FLAG)) {
  const trayEntry = join(process.resourcesPath, 'app-server', 'dist', 'trayBootstrap.js');
  if (!existsSync(trayEntry)) throw new Error('The packaged Beale app-server tray entry is missing.');
  void import(pathToFileURL(trayEntry).href).catch((error: unknown) => {
    process.stderr.write(`The packaged Beale app-server tray could not start: ${error instanceof Error ? error.message : String(error)}\n`);
    app.exit(1);
  });
} else {
  void import('./index');
}
