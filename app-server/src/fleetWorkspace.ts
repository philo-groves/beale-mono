import { createHash, randomUUID } from 'node:crypto';
import { closeSync, existsSync, lstatSync, mkdirSync, openSync, readFileSync, readSync, readdirSync, renameSync, rmSync, writeFileSync, writeSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import type { AppServerHostWorkspace } from './hostRegistry.js';
import { AppServerHostRegistry } from './hostRegistry.js';
import { initializeWorkspaceProjectAsync } from './workspaceCheckpoints.js';

const CHUNK_BYTES = 192 * 1024;
const MAX_FILE_BYTES = 2 * 1024 * 1024 * 1024;
const MAX_FILES = 100_000;

export interface FleetFile { path: string; size: number; sha256: string }
export interface FleetTransferBaseline {
  workspaceId: string;
  workspacePath: string;
  machineId: string;
  runId: string;
  ownerMachineId?: string;
  remoteServerId?: string;
  remoteMachineId?: string;
  remoteCompleted?: boolean;
  completedResult?: { imported: number; conflicts: number; candidateRecords: number };
  files: Record<string, string>;
}

export class FleetWorkspaceStore {
  private readonly workspaceDirectory: string;
  private readonly baselineDirectory: string;

  public constructor(private readonly registry: AppServerHostRegistry) {
    this.workspaceDirectory = join(registry.registryDirectory, 'fleet-workspaces');
    this.baselineDirectory = join(registry.registryDirectory, 'fleet-transfers');
  }

  public async guestStage(input: Record<string, unknown>): Promise<unknown> {
    const action = text(input.action, 'Fleet stage action');
    const workspaceId = safeId(input.workspaceId);
    const root = join(this.workspaceDirectory, workspaceId);
    if (action === 'begin') {
      mkdirSync(root, { recursive: true, mode: 0o700 });
      return { workspaceId, files: listFiles(root) };
    }
    if (action === 'write') {
      const path = safeChild(root, text(input.path, 'Fleet file path'));
      const offset = input.offset;
      const data = input.data;
      if (!Number.isSafeInteger(offset) || Number(offset) < 0 || typeof data !== 'string' || data.length > CHUNK_BYTES * 2) {
        throw new Error('Invalid Fleet file chunk.');
      }
      const bytes = Buffer.from(data, 'base64');
      if (bytes.toString('base64') !== data || bytes.length > CHUNK_BYTES || Number(offset) + bytes.length > MAX_FILE_BYTES) {
        throw new Error('Invalid Fleet file chunk encoding or size.');
      }
      safeParents(root, path);
      if (Number(offset) === 0) writeFileSync(path, bytes, { mode: 0o600 });
      else {
        if (!existsSync(path) || lstatSync(path).isSymbolicLink() || lstatSync(path).size !== Number(offset)) throw new Error('Fleet file chunk is out of sequence.');
        writeFileSync(path, bytes, { flag: 'a', mode: 0o600 });
      }
      return { written: bytes.length };
    }
    if (action === 'seal') {
      const path = safeChild(root, text(input.path, 'Fleet file path'));
      assertNoSymlinkPath(root, path);
      if (!existsSync(path) || !lstatSync(path).isFile() || typeof input.sha256 !== 'string'
        || !/^[a-f0-9]{64}$/u.test(input.sha256) || hashFile(path) !== input.sha256) {
        throw new Error('Fleet staged file failed its integrity check.');
      }
      return { verified: true };
    }
    if (action === 'finish') {
      const projectPath = join(root, 'workspace.json');
      assertNoSymlinkPath(root, projectPath);
      if (!existsSync(projectPath)) throw new Error('Fleet workspace metadata was not staged.');
      const project: unknown = JSON.parse(readFileSync(projectPath, 'utf8'));
      if (!record(project) || project.workspaceId !== workspaceId) throw new Error('Fleet workspace identity does not match the staged files.');
      const name = text(input.name, 'Fleet workspace name');
      const researchKitId = text(input.researchKitId, 'Fleet research kit');
      if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u.test(researchKitId)) throw new Error('Invalid Fleet research kit.');
      await initializeWorkspaceProjectAsync(root, workspaceId);
      this.registry.registerFleetWorkspace(root, workspaceId, name, researchKitId);
      return { workspaceId };
    }
    throw new Error('Unsupported Fleet stage action.');
  }

  public guestExport(input: Record<string, unknown>): unknown {
    const workspaceId = safeId(input.workspaceId);
    const root = join(this.workspaceDirectory, workspaceId);
    if (!existsSync(root)) throw new Error('Fleet workspace is unavailable.');
    const action = text(input.action, 'Fleet export action');
    if (action === 'list') return { files: listFiles(root) };
    if (action === 'read') {
      const path = safeChild(root, text(input.path, 'Fleet file path'));
      assertNoSymlinkPath(root, path);
      const offset = input.offset;
      if (!Number.isSafeInteger(offset) || Number(offset) < 0 || !existsSync(path) || !lstatSync(path).isFile()) throw new Error('Invalid Fleet export request.');
      const size = lstatSync(path).size;
      if (size > MAX_FILE_BYTES) throw new Error('Fleet file exceeds transfer limit.');
      return { data: readChunk(path, Number(offset)), size };
    }
    throw new Error('Unsupported Fleet export action.');
  }

  public sourceFiles(workspace: AppServerHostWorkspace): FleetFile[] {
    return listFiles(workspace.workspacePath);
  }

  public readSourceChunk(workspacePath: string, relativePath: string, offset: number): string {
    const path = safeChild(workspacePath, relativePath);
    assertNoSymlinkPath(workspacePath, path);
    if (!lstatSync(path).isFile()) throw new Error('Fleet source changed during transfer.');
    if (lstatSync(path).size > MAX_FILE_BYTES) throw new Error('Fleet file exceeds transfer limit.');
    return readChunk(path, offset);
  }

  public saveBaseline(baseline: FleetTransferBaseline): void {
    mkdirSync(this.baselineDirectory, { recursive: true, mode: 0o700 });
    const path = join(this.baselineDirectory, safeId(baseline.runId) + '.json');
    const temporary = `${path}.${randomUUID()}.tmp`;
    writeFileSync(temporary, JSON.stringify(baseline), { mode: 0o600 });
    renameSync(temporary, path);
  }

  public readBaseline(runId: string): FleetTransferBaseline {
    const path = join(this.baselineDirectory, safeId(runId) + '.json');
    const value: unknown = JSON.parse(readFileSync(path, 'utf8'));
    if (!record(value) || typeof value.workspacePath !== 'string' || typeof value.workspaceId !== 'string'
      || typeof value.machineId !== 'string' || typeof value.runId !== 'string' || !record(value.files)) {
      throw new Error('Fleet transfer baseline is invalid.');
    }
    return value as unknown as FleetTransferBaseline;
  }

  public async importGuestFile(
    baseline: FleetTransferBaseline, file: FleetFile, read: (offset: number) => Promise<string>,
  ): Promise<'imported' | 'conflict' | 'candidate'> {
    const root = baseline.workspacePath;
    const destination = safeChild(root, file.path);
    assertNoSymlinkPath(root, destination, true);
    const currentHash = existsSync(destination) && lstatSync(destination).isFile() ? hashFile(destination) : null;
    const originalHash = baseline.files[file.path] ?? null;
    const conflict = currentHash !== originalHash && currentHash !== file.sha256;
    const candidate = isManagedResearchRecord(file.path);
    const outputRoot = candidate
      ? join(root, '.beale', 'fleet-guest-records', safeId(baseline.runId))
      : conflict ? join(root, '.beale', 'fleet-conflicts', safeId(baseline.runId)) : root;
    const output = candidate || conflict ? safeChild(outputRoot, file.path) : destination;
    assertNoSymlinkPath(root, outputRoot, true);
    safeParents(outputRoot, output);
    const temporary = `${output}.${randomUUID()}.tmp`;
    const descriptor = openSync(temporary, 'w', 0o600);
    const digest = createHash('sha256');
    let received = 0;
    try {
      for (let offset = 0; offset < file.size; offset += CHUNK_BYTES) {
        const encoded = await read(offset);
        const bytes = Buffer.from(encoded, 'base64');
        if (bytes.toString('base64') !== encoded || bytes.length === 0 || bytes.length > CHUNK_BYTES) throw new Error('Fleet result chunk is invalid.');
        writeSync(descriptor, bytes);
        digest.update(bytes);
        received += bytes.length;
      }
    } catch (error) {
      closeSync(descriptor);
      rmSync(temporary, { force: true });
      throw error;
    }
    closeSync(descriptor);
    if (received !== file.size || digest.digest('hex') !== file.sha256) {
      rmSync(temporary, { force: true });
      throw new Error('Fleet result integrity check failed.');
    }
    renameSync(temporary, output);
    return candidate ? 'candidate' : conflict ? 'conflict' : 'imported';
  }
}

function listFiles(root: string): FleetFile[] {
  if (lstatSync(root).isSymbolicLink()) throw new Error('Fleet cannot transfer a symlinked workspace root.');
  const files: FleetFile[] = [];
  function visit(directory: string): void {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (entry.name === '.beale' || entry.name === '.git') continue;
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) throw new Error('Fleet cannot transfer workspace symlinks.');
      if (entry.isDirectory()) { visit(path); continue; }
      if (!entry.isFile()) throw new Error('Fleet cannot transfer special workspace files.');
      const size = lstatSync(path).size;
      if (size > MAX_FILE_BYTES) throw new Error('Fleet workspace contains a file above the transfer limit.');
      const relativePath = relative(root, path).split(sep).join('/');
      files.push({ path: relativePath, size, sha256: hashFile(path) });
      if (files.length > MAX_FILES) throw new Error('Fleet workspace contains too many files to transfer.');
    }
  }
  visit(root);
  return files.sort((a, b) => a.path.localeCompare(b.path));
}

function safeChild(root: string, relativePath: string): string {
  if (!relativePath || isAbsolute(relativePath) || relativePath.includes('\\')
    || relativePath.split('/').some((part) => !part || part === '.' || part === '..' || part === '.beale' || part === '.git')) {
    throw new Error('Invalid Fleet workspace file path.');
  }
  const path = resolve(root, relativePath);
  if (!path.startsWith(resolve(root) + sep)) throw new Error('Fleet workspace file escapes its root.');
  return path;
}

export function isManagedResearchRecord(path: string): boolean {
  return /^(claims|memories|runbooks|reports)\//u.test(path);
}

function safeParents(root: string, path: string): void {
  if (existsSync(root) && lstatSync(root).isSymbolicLink()) throw new Error('Fleet workspace root is a symlink.');
  mkdirSync(root, { recursive: true, mode: 0o700 });
  const parent = dirname(path);
  const relativeParent = relative(root, parent);
  let current = root;
  for (const part of relativeParent.split(sep).filter(Boolean)) {
    current = join(current, part);
    if (existsSync(current) && lstatSync(current).isSymbolicLink()) throw new Error('Fleet workspace path contains a symlink.');
    mkdirSync(current, { recursive: true, mode: 0o700 });
  }
  if (existsSync(path) && lstatSync(path).isSymbolicLink()) throw new Error('Fleet workspace file is a symlink.');
}

function assertNoSymlinkPath(root: string, path: string, allowMissing = false): void {
  let current = resolve(root);
  if (existsSync(current) && lstatSync(current).isSymbolicLink()) throw new Error('Fleet workspace root is a symlink.');
  for (const part of relative(current, path).split(sep).filter(Boolean)) {
    current = join(current, part);
    if (!existsSync(current)) {
      if (allowMissing) return;
      throw new Error('Fleet workspace file is unavailable.');
    }
    if (lstatSync(current).isSymbolicLink()) throw new Error('Fleet workspace path contains a symlink.');
  }
}

function hashFile(path: string): string {
  const digest = createHash('sha256');
  const descriptor = openSync(path, 'r');
  const buffer = Buffer.allocUnsafe(CHUNK_BYTES);
  try {
    for (;;) {
      const count = readSync(descriptor, buffer, 0, buffer.length, null);
      if (count === 0) break;
      digest.update(buffer.subarray(0, count));
    }
  } finally { closeSync(descriptor); }
  return digest.digest('hex');
}
function readChunk(path: string, offset: number): string {
  const descriptor = openSync(path, 'r');
  const bytes = Buffer.allocUnsafe(CHUNK_BYTES);
  try {
    const count = readSync(descriptor, bytes, 0, bytes.length, offset);
    return bytes.subarray(0, count).toString('base64');
  } finally { closeSync(descriptor); }
}
function record(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null && !Array.isArray(value); }
function text(value: unknown, label: string): string { if (typeof value !== 'string' || !value.trim()) throw new Error(`${label} is required.`); return value.trim(); }
function safeId(value: unknown): string { const id = text(value, 'Fleet workspace ID'); if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u.test(id)) throw new Error('Invalid Fleet workspace ID.'); return id; }
