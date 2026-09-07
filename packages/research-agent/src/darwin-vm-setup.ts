import { existsSync, readFileSync, realpathSync, renameSync, statSync, writeFileSync } from 'node:fs';
import { isAbsolute, join, relative } from 'node:path';
import { randomUUID } from 'node:crypto';

export interface DarwinVmSetupState {
  enabled: boolean;
  neverPrompt: boolean;
  checkoutRoot: string | null;
  prepared: boolean;
  errors: string[];
  hostPlatform: string;
}

export interface DarwinVmSetupUpdate {
  neverPrompt?: boolean;
  checkoutRoot?: string;
}

interface SavedSetup { neverPrompt: boolean; checkoutRoot: string | null }

export function readDarwinVmSetup(directory: string): SavedSetup {
  const path = join(directory, 'darwin-vm-setup.json');
  if (!existsSync(path)) return { neverPrompt: false, checkoutRoot: null };
  const value: unknown = JSON.parse(readFileSync(path, 'utf8'));
  if (!value || typeof value !== 'object') throw new Error('Invalid Darwin VM setup preferences.');
  const saved = value as Record<string, unknown>;
  return {
    neverPrompt: saved.neverPrompt === true,
    checkoutRoot: typeof saved.checkoutRoot === 'string' ? saved.checkoutRoot : null
  };
}

export function inspectDarwinVmSetup(checkoutRoot: string): string[] {
  if (!isAbsolute(checkoutRoot)) return ['Choose an absolute checkout path on the app-server host.'];
  try {
    const root = realpathSync(checkoutRoot);
    const required = [
      'qemu-sptm/build/qemu-system-aarch64', 'firmware/bootkc', 'firmware/dtree',
      'firmware/ramdisk.tc', 'firmware/ramdisk.dmg'
    ];
    for (const file of ['firmware/sptm', 'firmware/txm']) {
      if (existsSync(join(root, file))) required.push(file);
    }
    const errors: string[] = [];
    for (const file of required) {
      try {
        const path = realpathSync(join(root, file));
        const child = relative(root, path);
        const stat = statSync(path);
        if (child.startsWith('..') || isAbsolute(child) || !stat.isFile() || stat.size === 0) {
          errors.push(`Invalid or empty artifact: ${file}.`);
        }
      } catch { errors.push(`Missing required artifact: ${file}.`); }
    }
    if (existsSync(join(root, 'firmware/sptm')) !== existsSync(join(root, 'firmware/txm'))) {
      errors.push('Supply SPTM and TXM together, or remove the incomplete pair.');
    }
    return errors;
  } catch { return ['The checkout directory is unavailable on the app-server host.']; }
}

export function saveDarwinVmSetup(directory: string, update: DarwinVmSetupUpdate): void {
  if (!update || typeof update !== 'object' || Array.isArray(update)
    || (update.neverPrompt !== undefined && typeof update.neverPrompt !== 'boolean')
    || (update.checkoutRoot !== undefined && typeof update.checkoutRoot !== 'string')) {
    throw new Error('Invalid Darwin VM setup update.');
  }
  const saved = readDarwinVmSetup(directory);
  if (update.checkoutRoot !== undefined) {
    const errors = inspectDarwinVmSetup(update.checkoutRoot);
    if (errors.length) throw new Error(errors.join(' '));
    saved.checkoutRoot = realpathSync(update.checkoutRoot);
  }
  if (update.neverPrompt !== undefined) saved.neverPrompt = update.neverPrompt;
  const path = join(directory, 'darwin-vm-setup.json');
  const temporary = `${path}.${randomUUID()}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(saved)}\n`, { encoding: 'utf8', mode: 0o600 });
  renameSync(temporary, path);
}
