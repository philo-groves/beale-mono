import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

export interface AppServerInvocation {
  command: string;
  prefixArgs: string[];
  cwd: string;
  configuredBy: 'env_command' | 'env_root' | 'workspace_root';
  usesNodeRuntime: boolean;
}

/**
 * Packaged installers stage the app-server runtime in Electron's resources
 * directory with the same layout as the monorepo (see
 * apps/desktop/scripts/stage-runtime.mjs). Returns that directory, or null when
 * running from a source checkout, where Electron's resources hold no runtime.
 */
export function packagedAppServerRuntimeRoot(): string | null {
  const resourcesPath = (process as NodeJS.Process & { resourcesPath?: string }).resourcesPath;
  if (!resourcesPath) return null;
  return existsSync(join(resourcesPath, 'packages', 'app-server-runtime', 'dist', 'cli.js')) ? resourcesPath : null;
}

/**
 * Locate the root that carries the optional app-server client at
 * packages/app-server-runtime/dist/cli.js: the packaged runtime when present,
 * otherwise the monorepo root found by walking up from the current directory.
 * Returns null when neither exists.
 */
export function resolveAppServerWorkspaceRoot(): string | null {
  const packagedRoot = packagedAppServerRuntimeRoot();
  if (packagedRoot) return packagedRoot;
  let directory = process.cwd();
  for (;;) {
    if (existsSync(join(directory, 'packages', 'app-server-runtime', 'dist', 'cli.js'))) return directory;
    if (existsSync(join(directory, 'pnpm-workspace.yaml'))) return directory;
    const parent = resolve(directory, '..');
    if (parent === directory) return null;
    directory = parent;
  }
}

function defaultAppServerRoot(): string {
  return resolveAppServerWorkspaceRoot() || resolve(process.cwd(), '..', 'app-server');
}

export function resolveAppServerInvocation(): AppServerInvocation {
  const command = process.env.BEALE_APP_SERVER_COMMAND?.trim();
  if (command) {
    return {
      command,
      prefixArgs: parseEnvArgs('BEALE_APP_SERVER_ARGS_JSON'),
      cwd: process.env.BEALE_APP_SERVER_CWD?.trim() || process.cwd(),
      configuredBy: 'env_command',
      usesNodeRuntime: isPlainNodeExecutable(command)
    };
  }

  const configuredRoot = process.env.BEALE_APP_SERVER_ROOT?.trim();
  const root = configuredRoot || defaultAppServerRoot();
  const cliPath = join(root, 'packages', 'app-server-runtime', 'dist', 'cli.js');
  if (existsSync(cliPath)) {
    return {
      command: resolveAppServerNodeCommand(),
      prefixArgs: [cliPath],
      cwd: root,
      configuredBy: configuredRoot ? 'env_root' : 'workspace_root',
      usesNodeRuntime: true
    };
  }
  return {
    command: process.env.BEALE_APP_SERVER_PNPM_COMMAND?.trim() || 'pnpm',
    prefixArgs: ['--dir', root, 'start'],
    cwd: root,
    configuredBy: configuredRoot ? 'env_root' : 'workspace_root',
    usesNodeRuntime: false
  };
}

/**
 * Legacy synchronous call sites use the optional app-server client. Async
 * feature operations call app-server directly. A custom compatibility client
 * must opt in through the dedicated variables and implement protocol v1.
 */
export function resolveAppServerProtocolInvocation(): AppServerInvocation {
  const command = process.env.BEALE_APP_SERVER_PROTOCOL_COMMAND?.trim();
  if (command) {
    return {
      command,
      prefixArgs: parseEnvArgs('BEALE_APP_SERVER_PROTOCOL_ARGS_JSON'),
      cwd: process.env.BEALE_APP_SERVER_PROTOCOL_CWD?.trim() || process.cwd(),
      configuredBy: 'env_command',
      usesNodeRuntime: isPlainNodeExecutable(command)
    };
  }
  const root = process.env.BEALE_APP_SERVER_PROTOCOL_ROOT?.trim()
    || process.env.BEALE_APP_SERVER_ROOT?.trim()
    || defaultAppServerRoot();
  const cliPath = join(root, 'packages', 'app-server-runtime', 'dist', 'cli.js');
  if (existsSync(cliPath)) {
    return {
      command: process.env.BEALE_APP_SERVER_PROTOCOL_NODE_COMMAND?.trim() || resolveAppServerNodeCommand(),
      prefixArgs: [cliPath],
      cwd: root,
      configuredBy: process.env.BEALE_APP_SERVER_PROTOCOL_ROOT || process.env.BEALE_APP_SERVER_ROOT ? 'env_root' : 'workspace_root',
      usesNodeRuntime: true
    };
  }
  return {
    command: process.env.BEALE_APP_SERVER_PROTOCOL_PNPM_COMMAND?.trim()
      || process.env.BEALE_APP_SERVER_PNPM_COMMAND?.trim()
      || 'pnpm',
    prefixArgs: ['--dir', root, 'start'],
    cwd: root,
    configuredBy: process.env.BEALE_APP_SERVER_PROTOCOL_ROOT || process.env.BEALE_APP_SERVER_ROOT ? 'env_root' : 'workspace_root',
    usesNodeRuntime: false
  };
}

export function resolveAppServerNodeCommand(): string {
  const configured = [
    process.env.BEALE_APP_SERVER_NODE_COMMAND?.trim(),
    process.env.BEALE_NODE_COMMAND?.trim()
  ];
  // An installed build cannot assume a system Node.js (GUI launches often lack
  // the shell PATH, and an old Node may lack built-in SQLite). It runs the bundled
  // runtime with Beale's own Electron binary instead; see appServerNodeEnvironment.
  const candidates = packagedAppServerRuntimeRoot()
    ? configured
    : [
        ...configured,
        process.env.npm_node_execpath?.trim(),
        process.env.NODE?.trim(),
        'node',
        isPlainNodeExecutable(process.execPath) ? process.execPath : ''
      ];
  const seen = new Set<string>();
  for (const candidate of candidates) {
    if (!candidate || seen.has(candidate)) continue;
    seen.add(candidate);
    if (nodeCommandAvailable(candidate)) return candidate;
  }
  return packagedAppServerRuntimeRoot() ? process.execPath : 'node';
}

/**
 * Electron's binary only behaves as Node.js when ELECTRON_RUN_AS_NODE is set.
 * Every spawn that may receive process.execPath from resolveAppServerNodeCommand
 * must pass its environment through this helper; without it the child would
 * start a second Beale window instead of the app-server.
 */
export function appServerNodeEnvironment(command: string, env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  return command === process.execPath && process.versions.electron ? { ...env, ELECTRON_RUN_AS_NODE: '1' } : env;
}

function nodeCommandAvailable(command: string): boolean {
  const result = spawnSync(command, ['--version'], {
    encoding: 'utf8',
    timeout: 3000,
    windowsHide: true
  });
  return result.status === 0 && /^v\d+\.\d+\.\d+/.test(result.stdout.trim());
}

function isPlainNodeExecutable(path: string): boolean {
  const name = path.split(/[\\/]+/).at(-1)?.toLowerCase() ?? '';
  return name === 'node' || name === 'node.exe';
}

function parseEnvArgs(name: string): string[] {
  const raw = process.env[name]?.trim();
  if (!raw) return [];
  const parsed = JSON.parse(raw) as unknown;
  if (!Array.isArray(parsed) || parsed.some((value) => typeof value !== 'string')) {
    throw new Error(`${name} must be a JSON array of strings.`);
  }
  return parsed;
}
