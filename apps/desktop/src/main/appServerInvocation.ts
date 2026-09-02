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
 * Walk up from the current directory to locate the enclosing monorepo root
 * that carries the optional app-server client at packages/app-server-runtime/dist/cli.js. Returns null
 * when no workspace root is found.
 */
export function resolveAppServerWorkspaceRoot(): string | null {
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
  const candidates = [
    process.env.BEALE_APP_SERVER_NODE_COMMAND?.trim(),
    process.env.BEALE_NODE_COMMAND?.trim(),
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
  return 'node';
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
