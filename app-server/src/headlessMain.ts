import { pathToFileURL } from 'node:url';
import {
  acquireDiscoveryLock,
  defaultDiscoveryPath,
  isProcessAlive,
  readDiscoveryRecord,
  releaseDiscoveryLock
} from './discovery.js';
import { startAppServer, type AppServerOptions } from './appServer.js';

export async function runHeadlessMain(args: readonly string[] = process.argv.slice(2)): Promise<number> {
  const parsed = parseArgs(args);
  if (!parsed.ok) {
    process.stdout.write(`${parsed.error}\n\n${USAGE}\n`);
    return 2;
  }
  const options = parsed.value;
  if (options.help) {
    process.stdout.write(USAGE);
    return 0;
  }

  const stateFile = options.stateFile ?? process.env.BEALE_APP_SERVER_STATE_FILE?.trim() ?? defaultDiscoveryPath();
  const existing = readDiscoveryRecord(stateFile);
  if (existing && isProcessAlive(existing.pid)) {
    process.stdout.write(
      `Beale App Server is already running (pid ${existing.pid}) at ${existing.url}. Stop it first or remove ${stateFile}.\n`
    );
    return 1;
  }
  if (!acquireDiscoveryLock(stateFile, process.pid)) {
    process.stdout.write(`Another Beale App Server launch owns ${stateFile}.\n`);
    return 1;
  }

  const host = options.host ?? process.env.BEALE_APP_SERVER_HOST?.trim() ?? '127.0.0.1';
  const portEnv = Number.parseInt(process.env.BEALE_APP_SERVER_PORT ?? '', 10);
  const port = options.port ?? (Number.isInteger(portEnv) && portEnv > 0 ? portEnv : 0);
  const operatorToken = process.env.BEALE_APP_SERVER_TOKEN?.trim() || undefined;
  const publicUrl = options.publicUrl ?? (process.env.BEALE_APP_SERVER_PUBLIC_URL?.trim() || undefined);

  let requestShutdown = (): void => undefined;
  const serverOptions: AppServerOptions = {
    host,
    port,
    hostMode: 'headless',
    recoverInterruptedOnStart: true,
    ...(publicUrl ? { publicUrl } : {}),
    discoveryFile: stateFile,
    onShutdownRequested: () => requestShutdown()
  };
  if (operatorToken) serverOptions.operatorToken = operatorToken;

  const server = await startAppServer(serverOptions).catch((error: unknown) => {
    releaseDiscoveryLock(stateFile, process.pid);
    throw error;
  });
  process.stdout.write(`Beale App Server listening at ${server.url} (pid ${process.pid}).\n`);
  process.stdout.write(`Endpoint record: ${stateFile}\n`);

  let closing = false;
  const shutdown = (signal: string): void => {
    if (closing) return;
    closing = true;
    clearInterval(lifetimeMonitor);
    process.stdout.write(`Received ${signal}; stopping sessions and shutting down.\n`);
    const forcedExit = setTimeout(() => process.exit(1), 5_000);
    void server.close().then(
      () => {
        clearTimeout(forcedExit);
        releaseDiscoveryLock(stateFile, process.pid);
        process.exit(0);
      },
      () => {
        clearTimeout(forcedExit);
        releaseDiscoveryLock(stateFile, process.pid);
        process.exit(1);
      }
    );
  };
  const parentPid = ephemeralParentPid(process.env.BEALE_APP_SERVER_PARENT_PID);
  const lifetimeMonitor = setInterval(() => {
    const discovery = readDiscoveryRecord(stateFile);
    if (!discovery || discovery.pid !== process.pid) {
      shutdown('discovery ownership lost');
      return;
    }
    if (parentPid && !isProcessAlive(parentPid)) shutdown('ephemeral parent exited');
  }, 1_000);
  lifetimeMonitor.unref();
  requestShutdown = () => shutdown('control-plane request');
  process.once('SIGINT', () => shutdown('SIGINT'));
  process.once('SIGTERM', () => shutdown('SIGTERM'));
  process.once('SIGHUP' as NodeJS.Signals, () => shutdown('SIGHUP'));

  if (options.check) {
    setTimeout(() => {
      process.stdout.write('--check complete; stopping.\n');
      shutdown('--check complete');
    }, 250);
  }
  return 0;
}

function ephemeralParentPid(value: string | undefined): number | null {
  const pid = Number.parseInt(value?.trim() ?? '', 10);
  return Number.isInteger(pid) && pid > 0 && pid !== process.pid ? pid : null;
}

interface ParsedHeadlessArgs {
  ok: false;
  error: string;
}

interface ValidatedHeadlessArgs {
  ok: true;
  value: {
    help: boolean;
    check: boolean;
    host?: string;
    port?: number;
    publicUrl?: string;
    stateFile?: string;
  };
}

function parseArgs(args: readonly string[]): ParsedHeadlessArgs | ValidatedHeadlessArgs {
  const value: ValidatedHeadlessArgs['value'] = {
    help: false,
    check: false
  };
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === '--help' || arg === '-h') {
      value.help = true;
    } else if (arg === '--check') {
      value.check = true;
    } else if (arg === '--host') {
      const next = args[index + 1];
      index += 1;
      if (!next) return { ok: false, error: '--host requires a value.' };
      value.host = next;
    } else if (arg === '--port') {
      const next = args[index + 1];
      index += 1;
      const port = Number.parseInt(next ?? '', 10);
      if (!next || !Number.isInteger(port) || port <= 0 || port > 65_535) {
        return { ok: false, error: '--port requires an integer between 1 and 65535.' };
      }
      value.port = port;
    } else if (arg === '--state-file') {
      const next = args[index + 1];
      index += 1;
      if (!next) return { ok: false, error: '--state-file requires a path.' };
      value.stateFile = next;
    } else if (arg === '--public-url') {
      const next = args[index + 1];
      index += 1;
      if (!next) return { ok: false, error: '--public-url requires a value.' };
      value.publicUrl = next;
    } else {
      return { ok: false, error: `Unknown argument: ${arg}` };
    }
  }
  return { ok: true, value };
}

const USAGE = `Beale App Server (headless)

Usage:
  node dist/headlessMain.js [options]

Options:
  --host <address>     Bind address (default 127.0.0.1).
  --port <port>        TCP port (default: ephemeral).
  --public-url <url>   Public HTTP(S) origin advertised to clients.
  --state-file <path>  Discovery record path (default ~/.beale/app-server.json,
                       override with BEALE_APP_SERVER_STATE_FILE).
  --check              Start, confirm the listener, then exit.
  -h, --help           Show this message.

Environment:
  BEALE_APP_SERVER_HOST        Bind address.
  BEALE_APP_SERVER_PORT        TCP port.
  BEALE_APP_SERVER_PUBLIC_URL  Public HTTP(S) origin, including Tailscale Serve.
  BEALE_APP_SERVER_TOKEN       Explicit operator token override (otherwise the
                               persistent sibling .token file is used).
  BEALE_APP_SERVER_STATE_FILE  Discovery record path.
  BEALE_APP_SERVER_PARENT_PID  Optional ephemeral owner PID; the headless host
                               exits when this process is no longer alive.
`;

const invokedDirectly = process.argv[1]
  ? pathToFileURL(process.argv[1]).href === import.meta.url
  : false;
if (invokedDirectly) {
  void runHeadlessMain().then((code) => {
    if (code !== 0) process.exitCode = code;
  }, (error: unknown) => {
    process.stderr.write(`Beale App Server failed: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}
