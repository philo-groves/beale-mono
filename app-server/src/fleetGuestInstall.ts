import { execFile, spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream, copyFileSync, cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, readdirSync, rmSync, statSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { BEALE_APP_SERVER_CONTRACT_TIMESTAMP, BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT, type FleetBackend } from '@beale/app-server-runtime/protocol';
import { sshConnectionArgs } from './fleetSsh.js';

const execFileAsync = promisify(execFile);
const DEPLOY_TIMEOUT_MS = 5 * 60_000;
const SSH_TIMEOUT_MS = 90_000;
const TRANSFER_TIMEOUT_MS = 5 * 60_000;
const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');

export interface FleetGuestSshConnection {
  machine: { backend: FleetBackend };
  sshHost: string;
  sshUser: string;
  sshIdentityFile: string | null;
  sshKnownHostsFile: string | null;
}

interface GuestBundle { archive: string; buildId: string; sha256: string }

/** Installs only into a session VM's user account; never changes the base VM. */
export class FleetGuestInstaller {
  private bundlePromise: Promise<GuestBundle> | null = null;
  private buildIdPromise: Promise<string> | null = null;

  public async ensure(connection: FleetGuestSshConnection, forceRestart = false): Promise<void> {
    const args = sshConnectionArgs(connection.sshHost, connection.sshUser,
      connection.sshIdentityFile, connection.sshKnownHostsFile, connection.machine.backend);
    const buildId = await this.buildId();
    const guest = await this.probe(args, connection.machine.backend);
    if (guest.arch !== process.arch) {
      throw new Error(`Fleet guest architecture ${guest.arch} does not match this primary's ${process.arch} runtime.`);
    }
    if (guest.buildId !== buildId) {
      const bundle = await this.bundle();
      await transferArchive(args, connection.machine.backend, bundle);
      await sshScript(args, connection.machine.backend, installScript(connection.machine.backend, bundle), SSH_TIMEOUT_MS);
    } else {
      await sshScript(args, connection.machine.backend,
        `${forceRestart ? stopScript(connection.machine.backend, buildId) : ''}\n${startScript(connection.machine.backend, buildId)}`,
        SSH_TIMEOUT_MS);
    }
  }

  private async probe(args: string[], backend: FleetBackend): Promise<{ arch: string; buildId: string | null }> {
    const output = await sshScript(args, backend, backend === 'hyper-v'
      ? "$arch=$env:PROCESSOR_ARCHITECTURE; $path=Join-Path $HOME '.beale/fleet-runtime/version'; Write-Output $arch; if (Test-Path $path) { Write-Output ((Get-Content -Raw $path).Trim()) }"
      : "uname -m; if test -f \"$HOME/.beale/fleet-runtime/version\"; then cat \"$HOME/.beale/fleet-runtime/version\"; fi", SSH_TIMEOUT_MS);
    const lines = output.trim().split(/\r?\n/u).map((line) => line.trim()).filter(Boolean);
    const arch = normalizeArchitecture(lines[0] ?? '');
    if (!arch) throw new Error('Fleet could not determine the guest architecture over SSH.');
    const buildId = /^[a-f0-9]{24}$/u.test(lines[1] ?? '') ? lines[1]! : null;
    return { arch, buildId };
  }

  private buildId(): Promise<string> {
    this.buildIdPromise ??= Promise.resolve().then(() => {
      const hash = createHash('sha256').update(BEALE_APP_SERVER_CONTRACT_TIMESTAMP);
      if (existsSync(join(repoRoot, 'pnpm-workspace.yaml'))) {
        for (const directory of ['app-server/dist', 'app-server/resources',
          'packages/research-agent/dist', 'packages/app-server-runtime/dist']) {
          hashDirectory(join(repoRoot, directory), directory, hash);
        }
        hash.update(readFileSync(join(repoRoot, 'pnpm-lock.yaml')));
      } else {
        for (const directory of ['dist', 'resources', 'node_modules/@beale/research-agent/dist',
          'node_modules/@beale/app-server-runtime/dist']) {
          hashDirectory(join(packageRoot, directory), directory, hash);
        }
      }
      return hash.digest('hex').slice(0, 24);
    });
    return this.buildIdPromise;
  }

  private bundle(): Promise<GuestBundle> {
    this.bundlePromise ??= this.prepareBundle().catch((error) => {
      this.bundlePromise = null;
      throw error;
    });
    return this.bundlePromise;
  }

  private async prepareBundle(): Promise<GuestBundle> {
    const buildId = await this.buildId();
    const directory = mkdtempSync(join(tmpdir(), 'beale-fleet-guest-'));
    const deployment = join(directory, 'deployment');
    const archive = join(directory, `${buildId}.tar.gz`);
    try {
      let entry = '../dist/headlessMain.js';
      if (existsSync(join(repoRoot, 'pnpm-workspace.yaml'))) {
        try {
          await execFileAsync(process.env.BEALE_FLEET_PNPM_COMMAND?.trim() || 'pnpm', [
            '--pm-on-fail=ignore', '--filter', '@beale/app-server', 'deploy', '--legacy', '--prod',
            '--offline', '--trust-lockfile', deployment,
          ], { cwd: repoRoot, timeout: DEPLOY_TIMEOUT_MS, maxBuffer: 2_000_000, windowsHide: true,
            env: { ...process.env, CI: '1', npm_config_update_notifier: 'false' } });
        } catch (error) {
          const failure = error as NodeJS.ErrnoException & { stdout?: string; stderr?: string };
          const detail = `${failure.stdout ?? ''}\n${failure.stderr ?? ''}`;
          if (failure.code !== 'ENOENT' && !detail.includes('ERR_PNPM_NO_OFFLINE_TARBALL')) throw error;
          rmSync(deployment, { recursive: true, force: true });
          copyWorkspaceRuntime(deployment);
          entry = '../app-server/dist/headlessMain.js';
        }
      } else {
        copyDeployedRuntime(deployment);
      }
      if (!existsSync(join(deployment, entry.slice(3)))
        && !existsSync(join(deployment, 'app-server', 'dist', 'headlessMain.js'))) {
        throw new Error('The Fleet deployment is missing its built app-server entry. Build the workspace packages first.');
      }
      const node = await localNodeBinary();
      mkdirSync(join(deployment, 'bin'), { recursive: true });
      copyFileSync(node, join(deployment, 'bin', process.platform === 'win32' ? 'node.exe' : 'node'));
      writeFileSync(join(deployment, 'bin', 'start.mjs'), `import { runHeadlessMain } from ${JSON.stringify(entry)};\nvoid runHeadlessMain().then((code) => { if (code !== 0) process.exitCode = code; }, (error) => { process.stderr.write(String(error) + '\\n'); process.exitCode = 1; });\n`);
      await execFileAsync(node, [join(deployment, 'bin', 'start.mjs'), '--help'],
        { timeout: 30_000, maxBuffer: 100_000, windowsHide: true });
      writeFileSync(join(deployment, 'bin', 'links.mjs'), GUEST_LINK_INSTALLER);
      normalizeArchiveLinks(deployment);
      await execFileAsync(process.platform === 'win32' ? 'tar.exe' : 'tar', [
        '-czf', archive, '-C', deployment, '.',
      ], { timeout: DEPLOY_TIMEOUT_MS, maxBuffer: 2_000_000, windowsHide: true });
      const hash = createHash('sha256');
      for await (const chunk of createReadStream(archive)) hash.update(chunk);
      process.once('exit', () => { try { rmSync(directory, { recursive: true, force: true }); } catch { /* Best-effort cache cleanup. */ } });
      return { archive, buildId, sha256: hash.digest('hex') };
    } catch (error) {
      rmSync(directory, { recursive: true, force: true });
      throw error;
    } finally {
      rmSync(deployment, { recursive: true, force: true });
    }
  }
}

const GUEST_LINK_INSTALLER = `import { existsSync, mkdirSync, readFileSync, symlinkSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const links = JSON.parse(readFileSync(join(root, 'bin', 'links.json'), 'utf8'));
for (const { link, target } of links) {
  const destination = resolve(root, link);
  const source = resolve(root, target);
  for (const path of [destination, source]) {
    const inside = relative(root, path);
    if (inside.startsWith('..' + sep) || isAbsolute(inside)) throw new Error('Invalid Fleet runtime link.');
  }
  if (existsSync(destination)) continue;
  mkdirSync(dirname(destination), { recursive: true });
  symlinkSync(process.platform === 'win32' ? source : relative(dirname(destination), source), destination,
    process.platform === 'win32' ? 'junction' : 'dir');
}
`;

function normalizeArchiveLinks(root: string): void {
  const canonicalRoot = realpathSync(root);
  const canonicalPackageRoot = realpathSync(packageRoot);
  const canonicalRepoRoot = existsSync(repoRoot) ? realpathSync(repoRoot) : null;
  const links: Array<{ link: string; target: string; file: boolean }> = [];
  const visit = (directory: string): void => {
    for (const name of readdirSync(directory)) {
      const path = join(directory, name);
      const entry = lstatSync(path);
      if (entry.isSymbolicLink()) {
        const source = realpathSync(path);
        const target = [canonicalRoot, canonicalPackageRoot, canonicalRepoRoot]
          .filter((candidate): candidate is string => Boolean(candidate))
          .map((candidate) => relative(candidate, source))
          .find((candidate) => !candidate.startsWith(`..${sep}`) && !isAbsolute(candidate)
            && existsSync(join(root, candidate)));
        if (target === undefined) {
          throw new Error(`The Fleet deployment contains an external dependency link: ${relative(root, path)}.`);
        }
        links.push({ link: relative(root, path), target, file: statSync(source).isFile() });
      } else if (entry.isDirectory()) visit(path);
    }
  };
  visit(root);
  for (const link of links) {
    const path = join(root, link.link);
    unlinkSync(path);
    if (link.file) copyFileSync(join(root, link.target), path);
  }
  writeFileSync(join(root, 'bin', 'links.json'), JSON.stringify(links.filter((link) => !link.file)));
}

function copyDeployedRuntime(destination: string): void {
  if (!existsSync(join(packageRoot, 'node_modules'))) {
    throw new Error('The primary app-server has no installed dependency tree for Fleet installation.');
  }
  mkdirSync(destination, { recursive: true });
  for (const name of ['dist', 'resources', 'package.json', 'node_modules']) {
    cpSync(join(packageRoot, name), join(destination, name), { recursive: true, dereference: false, verbatimSymlinks: true });
  }
}

function copyWorkspaceRuntime(destination: string): void {
  const virtualRoot = join(repoRoot, 'node_modules', '.pnpm');
  if (!existsSync(virtualRoot)) throw new Error('Fleet guest installation needs installed workspace dependencies.');
  const pending: string[] = [];
  const copied = new Set<string>();
  for (const packagePath of ['app-server', 'packages/research-agent', 'packages/app-server-runtime']) {
    const source = join(repoRoot, packagePath);
    const target = join(destination, packagePath);
    mkdirSync(target, { recursive: true });
    for (const name of ['dist', 'package.json', ...(packagePath === 'app-server' ? ['resources'] : [])]) {
      cpSync(join(source, name), join(target, name), { recursive: true, dereference: false, verbatimSymlinks: true });
    }
    const manifest = JSON.parse(readFileSync(join(source, 'package.json'), 'utf8')) as { dependencies?: Record<string, string> };
    for (const name of Object.keys(manifest.dependencies ?? {})) {
      copyDependencyLink(join(source, 'node_modules', name), join(target, 'node_modules', name), virtualRoot, destination, pending);
    }
  }
  while (pending.length) {
    const id = pending.pop()!;
    if (copied.has(id)) continue;
    copied.add(id);
    const source = join(virtualRoot, id);
    cpSync(source, join(destination, 'node_modules', '.pnpm', id), { recursive: true, dereference: false, verbatimSymlinks: true });
    scanDependencyLinks(join(source, 'node_modules'), virtualRoot, pending);
  }
}

function copyDependencyLink(source: string, destination: string, virtualRoot: string, deployment: string, pending: string[]): void {
  if (!existsSync(source)) throw new Error(`Installed Fleet dependency is missing: ${basename(source)}.`);
  mkdirSync(dirname(destination), { recursive: true });
  const target = realpathSync(source);
  const workspaceRelative = relative(repoRoot, target);
  if (workspaceRelative.startsWith(`..${sep}`) || isAbsolute(workspaceRelative)) {
    throw new Error('Fleet dependency points outside the installed workspace.');
  }
  symlinkSync(process.platform === 'win32'
    ? join(deployment, workspaceRelative)
    : relative(dirname(destination), join(deployment, workspaceRelative)), destination,
    process.platform === 'win32' ? 'junction' : 'dir');
  enqueueVirtualPackage(target, virtualRoot, pending);
}

function scanDependencyLinks(directory: string, virtualRoot: string, pending: string[]): void {
  if (!existsSync(directory)) return;
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry);
    if (entry.startsWith('@') && lstatSync(path).isDirectory() && !lstatSync(path).isSymbolicLink()) {
      scanDependencyLinks(path, virtualRoot, pending);
    } else if (lstatSync(path).isSymbolicLink()) {
      enqueueVirtualPackage(realpathSync(path), virtualRoot, pending);
    }
  }
}

function enqueueVirtualPackage(path: string, virtualRoot: string, pending: string[]): void {
  const relativePath = relative(virtualRoot, path);
  if (relativePath.startsWith(`..${sep}`) || isAbsolute(relativePath)) return;
  const id = relativePath.split(sep)[0];
  if (id) pending.push(id);
}

function hashDirectory(path: string, label: string, hash: ReturnType<typeof createHash>): void {
  if (!existsSync(path)) throw new Error(`Fleet guest installation requires a built ${label} directory.`);
  for (const entry of readdirSync(path, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) hashDirectory(child, `${label}/${entry.name}`, hash);
    else if (entry.isFile()) hash.update(`${label}/${entry.name}\0`).update(readFileSync(child));
  }
}

async function localNodeBinary(): Promise<string> {
  if (/^node(?:\.exe)?$/iu.test(basename(process.execPath))) return realpathSync(process.execPath);
  const configured = process.env.BEALE_APP_SERVER_NODE_COMMAND?.trim() || process.env.BEALE_NODE_COMMAND?.trim();
  if (configured && existsSync(configured)) return realpathSync(configured);
  const { stdout } = await execFileAsync(process.platform === 'win32' ? 'where.exe' : 'which', ['node'],
    { timeout: 10_000, windowsHide: true });
  const candidate = stdout.trim().split(/\r?\n/u)[0];
  if (!candidate || !existsSync(candidate)) throw new Error('Fleet auto-install needs a local Node.js executable.');
  return realpathSync(candidate);
}

export function normalizeArchitecture(value: string): string | null {
  const normalized = value.toLowerCase();
  if (normalized === 'arm64' || normalized === 'aarch64') return 'arm64';
  if (normalized === 'x64' || normalized === 'x86_64' || normalized === 'amd64') return 'x64';
  return null;
}

async function sshScript(args: string[], backend: FleetBackend, script: string, timeout: number): Promise<string> {
  const command = backend === 'hyper-v'
    ? `powershell.exe -NoProfile -NonInteractive -EncodedCommand ${Buffer.from(script, 'utf16le').toString('base64')}`
    : `sh -c '${script.replaceAll("'", "'\\''")}'`;
  const { stdout } = await execFileAsync('ssh', [...args, command],
    { timeout, maxBuffer: 100_000, encoding: 'utf8', windowsHide: true });
  return stdout;
}

async function transferArchive(args: string[], backend: FleetBackend, bundle: GuestBundle): Promise<void> {
  const script = backend === 'hyper-v'
    ? "$root=Join-Path $HOME '.beale/fleet-runtime'; New-Item -ItemType Directory -Force $root | Out-Null; $file=[IO.File]::Create((Join-Path $root 'upload.tar.gz')); try { [Console]::OpenStandardInput().CopyTo($file) } finally { $file.Dispose() }"
    : 'set -eu; mkdir -p "$HOME/.beale/fleet-runtime"; cat > "$HOME/.beale/fleet-runtime/upload.tar.gz"';
  const command = backend === 'hyper-v'
    ? `powershell.exe -NoProfile -NonInteractive -EncodedCommand ${Buffer.from(script, 'utf16le').toString('base64')}`
    : `sh -c '${script.replaceAll("'", "'\\''")}'`;
  await new Promise<void>((resolveTransfer, rejectTransfer) => {
    const child = spawn('ssh', [...args, command], { stdio: ['pipe', 'ignore', 'pipe'], windowsHide: true });
    const source = createReadStream(bundle.archive);
    let diagnostic = '';
    const timer = setTimeout(() => child.kill(), TRANSFER_TIMEOUT_MS);
    source.on('error', rejectTransfer);
    child.on('error', rejectTransfer);
    child.stderr.on('data', (chunk: Buffer) => { diagnostic = (diagnostic + chunk.toString()).slice(-2_000); });
    child.on('exit', (code) => {
      clearTimeout(timer);
      if (code === 0) resolveTransfer();
      else rejectTransfer(new Error(`Fleet guest archive transfer failed${diagnostic ? `: ${diagnostic.trim()}` : '.'}`));
    });
    source.pipe(child.stdin);
  });
}

export function installScript(backend: FleetBackend, bundle: GuestBundle): string {
  const { buildId, sha256 } = bundle;
  if (!/^[a-f0-9]{24}$/u.test(buildId) || !/^[a-f0-9]{64}$/u.test(sha256)) {
    throw new Error('Invalid Fleet guest installation archive identity.');
  }
  if (backend === 'hyper-v') return `
$ErrorActionPreference='Stop'
$root=Join-Path $HOME '.beale/fleet-runtime'
$archive=Join-Path $root 'upload.tar.gz'
if ((Get-FileHash -Algorithm SHA256 $archive).Hash.ToLowerInvariant() -ne '${sha256}') { throw 'Fleet guest archive failed its integrity check.' }
$release=Join-Path $root 'releases/${buildId}'
New-Item -ItemType Directory -Force $release | Out-Null
tar.exe -xzf $archive -C $release
if ($LASTEXITCODE -ne 0) { throw 'Fleet guest archive extraction failed.' }
& (Join-Path $release 'bin/node.exe') (Join-Path $release 'bin/links.mjs')
if ($LASTEXITCODE -ne 0) { throw 'Fleet guest dependency links could not be installed.' }
${stopScript(backend, buildId)}
Set-Content -NoNewline -Path (Join-Path $root 'version') -Value '${buildId}'
Remove-Item $archive -Force
${startScript(backend, buildId)}
`;
  return `set -eu
root="$HOME/.beale/fleet-runtime"
archive="$root/upload.tar.gz"
test "$(shasum -a 256 "$archive" | cut -d ' ' -f 1)" = '${sha256}' || { echo 'Fleet guest archive failed its integrity check.' >&2; exit 1; }
release="$root/releases/${buildId}"
mkdir -p "$release"
tar -xzf "$archive" -C "$release"
chmod 700 "$release/bin/node"
"$release/bin/node" "$release/bin/links.mjs"
${stopScript(backend, buildId)}
printf '%s' '${buildId}' > "$root/version"
rm -f "$archive"
${startScript(backend, buildId)}
`;
}

function stopScript(backend: FleetBackend, buildId: string): string {
  if (backend === 'hyper-v') return `
$discovery=Join-Path $HOME '.beale/app-server.json'
$oldPid=$null
if (Test-Path $discovery) { try { $oldPid=(Get-Content -Raw $discovery | ConvertFrom-Json).pid } catch {} }
if ($oldPid -and (Get-Process -Id $oldPid -ErrorAction SilentlyContinue)) {
  $processInfo=Get-CimInstance Win32_Process -Filter "ProcessId = $oldPid" -ErrorAction SilentlyContinue
  if (-not $processInfo -or $processInfo.CommandLine -notmatch 'headlessMain[.]js|app-server|Beale App Server|fleet-runtime.*start[.]mjs') {
    throw 'The guest discovery PID belongs to another process; Fleet did not stop it.'
  }
  Stop-Process -Id $oldPid -ErrorAction SilentlyContinue
  Wait-Process -Id $oldPid -Timeout 10 -ErrorAction SilentlyContinue
  if (Get-Process -Id $oldPid -ErrorAction SilentlyContinue) { throw 'Guest app-server did not stop for Fleet installation.' }
}
$supervisorPids=Get-NetTCPConnection -LocalPort ${BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT} -State Listen -ErrorAction SilentlyContinue | Select-Object -ExpandProperty OwningProcess -Unique
foreach ($supervisorPid in $supervisorPids) {
  $processInfo=Get-CimInstance Win32_Process -Filter "ProcessId = $supervisorPid" -ErrorAction SilentlyContinue
  if ($processInfo -and $processInfo.CommandLine -match 'restartSupervisorMain[.]js') {
    Stop-Process -Id $supervisorPid -ErrorAction SilentlyContinue
    Wait-Process -Id $supervisorPid -Timeout 10 -ErrorAction SilentlyContinue
  }
}
`;
  return `discovery="$HOME/.beale/app-server.json"
if test -f "$discovery"; then
  old_pid=$("$HOME/.beale/fleet-runtime/releases/${buildId}/bin/node" -e 'try { const value = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8")); if (value.version === 1 && Number.isSafeInteger(value.pid) && value.pid > 0) process.stdout.write(String(value.pid)); } catch {}' "$discovery")
  if test -n "$old_pid" && kill -0 "$old_pid" 2>/dev/null; then
    old_command=$(ps -p "$old_pid" -o command= 2>/dev/null || true)
    case "$old_command" in *headlessMain.js*|*app-server*|*Beale*App*Server*|*fleet-runtime*start.mjs*) : ;; *) echo 'The guest discovery PID belongs to another process; Fleet did not stop it.' >&2; exit 1 ;; esac
    kill "$old_pid"
    attempt=0
    while kill -0 "$old_pid" 2>/dev/null && test "$attempt" -lt 20; do sleep 0.5; attempt=$((attempt + 1)); done
    if kill -0 "$old_pid" 2>/dev/null; then echo 'Guest app-server did not stop for Fleet installation.' >&2; exit 1; fi
  fi
fi
if command -v lsof >/dev/null 2>&1; then
  for supervisor_pid in $(lsof -nP -tiTCP:${BEALE_APP_SERVER_SUPERVISOR_LOCAL_PORT} -sTCP:LISTEN 2>/dev/null || true); do
    supervisor_command=$(ps -p "$supervisor_pid" -o command= 2>/dev/null || true)
    case "$supervisor_command" in *restartSupervisorMain.js*)
      kill "$supervisor_pid" 2>/dev/null || true
      attempt=0
      while kill -0 "$supervisor_pid" 2>/dev/null && test "$attempt" -lt 20; do sleep 0.5; attempt=$((attempt + 1)); done
      if kill -0 "$supervisor_pid" 2>/dev/null; then echo 'Guest app-server restart supervisor did not stop.' >&2; exit 1; fi
      ;; esac
  done
fi`;
}

export function startScript(backend: FleetBackend, buildId: string): string {
  if (!/^[a-f0-9]{24}$/u.test(buildId)) throw new Error('Invalid Fleet guest installation version.');
  if (backend === 'hyper-v') return `
$root=Join-Path $HOME '.beale/fleet-runtime'
$release=Join-Path $root 'releases/${buildId}'
$discovery=Join-Path $HOME '.beale/app-server.json'
$running=$false
if (Test-Path $discovery) { try { $pidValue=(Get-Content -Raw $discovery | ConvertFrom-Json).pid; if ($pidValue) { $running=[bool](Get-Process -Id $pidValue -ErrorAction SilentlyContinue) } } catch {} }
if (-not $running) { Start-Process -FilePath (Join-Path $release 'bin/node.exe') -ArgumentList (Join-Path $release 'bin/start.mjs') -WorkingDirectory $release -WindowStyle Hidden -RedirectStandardOutput (Join-Path $root 'stdout.log') -RedirectStandardError (Join-Path $root 'stderr.log') }
`;
  return `root="$HOME/.beale/fleet-runtime"
release="$root/releases/${buildId}"
discovery="$HOME/.beale/app-server.json"
running_pid=""
if test -f "$discovery"; then running_pid=$("$release/bin/node" -e 'try { const value = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8")); if (value.version === 1 && Number.isSafeInteger(value.pid) && value.pid > 0) process.stdout.write(String(value.pid)); } catch {}' "$discovery"); fi
if test -n "$running_pid" && kill -0 "$running_pid" 2>/dev/null; then :
else nohup "$release/bin/node" "$release/bin/start.mjs" > "$root/server.log" 2>&1 < /dev/null & fi`;
}
