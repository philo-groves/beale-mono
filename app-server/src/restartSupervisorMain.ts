import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { BEALE_APP_SERVER_SUPERVISOR_HTTPS_PORT, BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT } from '@beale/app-server-runtime/protocol';
import { startRestartSupervisor, type RestartSupervisorConfig } from './restartSupervisor.js';
import { readPersistedRemoteAccessLaunchOptions } from './remoteAccessConfig.js';
import { runTailscaleCommand } from './tailnetRemote.js';

export function launchRestartSupervisor(config: Omit<RestartSupervisorConfig, 'port' | 'command' | 'args'>): void {
  const launch: RestartSupervisorConfig = {
    ...config,
    port: BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT,
    command: process.execPath,
    args: process.argv.slice(1)
  };
  const environment = {
    ...process.env,
    BEALE_APP_SERVER_SUPERVISOR_CONFIG: JSON.stringify(launch),
    ...(process.versions.electron ? { ELECTRON_RUN_AS_NODE: '1' } : {})
  };
  const child = spawn(process.execPath, [fileURLToPath(new URL('./restartSupervisorMain.js', import.meta.url))], {
    detached: true, stdio: 'ignore', windowsHide: true, env: environment
  });
  child.once('error', () => undefined);
  child.unref();
  if (readPersistedRemoteAccessLaunchOptions()) {
    void runTailscaleCommand([
      'serve', '--bg', '--yes', `--https=${BEALE_APP_SERVER_SUPERVISOR_HTTPS_PORT}`,
      `http://127.0.0.1:${BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT}`
    ]).catch((error: unknown) => {
      process.stderr.write(`Beale app-server restart route is unavailable: ${error instanceof Error ? error.message : String(error)}\n`);
    });
  }
}

if (process.env.BEALE_APP_SERVER_SUPERVISOR_CONFIG) {
  try {
    const config = JSON.parse(process.env.BEALE_APP_SERVER_SUPERVISOR_CONFIG) as RestartSupervisorConfig;
    if (!config || typeof config.discoveryFile !== 'string' || typeof config.command !== 'string'
      || !Array.isArray(config.args) || !config.args.every((arg) => typeof arg === 'string')
      || (config.hostMode !== 'tray' && config.hostMode !== 'headless')
      || typeof config.operatorToken !== 'string' || config.port !== BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT) {
      throw new Error('Invalid app-server restart supervisor configuration.');
    }
    void startRestartSupervisor(config).catch((error: unknown) => {
      if ((error as NodeJS.ErrnoException).code !== 'EADDRINUSE') {
        process.stderr.write(`Beale app-server restart supervisor failed: ${error instanceof Error ? error.message : String(error)}\n`);
        process.exitCode = 1;
      }
    });
  } catch (error) {
    process.stderr.write(`Beale app-server restart supervisor configuration failed: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}
