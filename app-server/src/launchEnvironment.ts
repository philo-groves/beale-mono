import { lstatSync, readFileSync, rmSync } from 'node:fs';

export const APP_SERVER_LAUNCH_ENVIRONMENT_FLAG = '--beale-launch-environment-file';
const MAX_LAUNCH_ENVIRONMENT_BYTES = 2 * 1024 * 1024;

export function consumeAppServerLaunchEnvironment(
  argv: string[] = process.argv,
  environment: NodeJS.ProcessEnv = process.env,
): boolean {
  const flagIndex = argv.indexOf(APP_SERVER_LAUNCH_ENVIRONMENT_FLAG);
  if (flagIndex < 0) return false;
  const path = argv[flagIndex + 1];
  if (!path || path.startsWith('-')) {
    throw new Error(`${APP_SERVER_LAUNCH_ENVIRONMENT_FLAG} requires a file path.`);
  }

  try {
    const stat = lstatSync(path);
    if (!stat.isFile() || stat.isSymbolicLink()) {
      throw new Error('The Beale app-server launch environment must be a regular file.');
    }
    if (stat.size > MAX_LAUNCH_ENVIRONMENT_BYTES) {
      throw new Error('The Beale app-server launch environment is too large.');
    }
    if (process.platform !== 'win32') {
      if ((stat.mode & 0o077) !== 0) {
        throw new Error('The Beale app-server launch environment must not be accessible to other users.');
      }
      const uid = typeof process.getuid === 'function' ? process.getuid() : undefined;
      if (uid !== undefined && stat.uid !== uid) {
        throw new Error('The Beale app-server launch environment belongs to another user.');
      }
    }

    const parsed = JSON.parse(readFileSync(path, 'utf8')) as unknown;
    if (!isStringRecord(parsed)) {
      throw new Error('The Beale app-server launch environment must contain a JSON object of strings.');
    }
    for (const [name, value] of Object.entries(parsed)) environment[name] = value;
    argv.splice(flagIndex, 2);
    return true;
  } finally {
    rmSync(path, { force: true });
  }
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return typeof value === 'object'
    && value !== null
    && !Array.isArray(value)
    && Object.entries(value).every(([name, entry]) => name.length > 0 && typeof entry === 'string');
}
