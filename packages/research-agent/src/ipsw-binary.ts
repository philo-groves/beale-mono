import { randomUUID, createHash } from "node:crypto";
import { constants, createReadStream } from "node:fs";
import { access, chmod, copyFile, mkdir, open, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { unzipSync } from "fflate";
import { x as extractTar } from "tar";

const IPSW_VERSION = "3.1.739";
const MAX_ARCHIVE_BYTES = 100 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_MS = 90_000;
const LOCK_STALE_MS = 3 * 60_000;

interface IpswAsset {
  filename: string;
  sha256: string;
}

export interface IpswTarget {
  platform: NodeJS.Platform;
  arch: string;
}

// Pinned to the SHA-256 digests published with blacktop/ipsw v3.1.739.
const ASSETS: Record<string, IpswAsset> = {
  "darwin-arm64": { filename: "ipsw_3.1.739_macOS_arm64.tar.gz", sha256: "c09e8eba02c332f5b50d15f94383e59dd35646d70f6ea9e78d185e9afda6abc2" },
  "darwin-x64": { filename: "ipsw_3.1.739_macOS_x86_64.tar.gz", sha256: "7121fce3ff630077d7de0e3e6ff15f49876494b5d54b01f92a3c1787ebe23c0e" },
  "linux-arm64": { filename: "ipsw_3.1.739_linux_arm64.tar.gz", sha256: "1df59830692c44d7fc956eeba7e94234791f663fd74c0b16780c82808e6b1c62" },
  "linux-x64": { filename: "ipsw_3.1.739_linux_x86_64.tar.gz", sha256: "8124bdba7902e7627bae1bb2e1117c9cfcdc31de0f65ec55e341565ac6029a01" },
  "win32-arm64": { filename: "ipsw_3.1.739_windows_arm64.zip", sha256: "258205f64e624d8031526645a274e59e79663d0f1cbc4512f519e57705a9ff21" },
  "win32-x64": { filename: "ipsw_3.1.739_windows_x86_64.zip", sha256: "43d311cf87ffab0617845b10fbf45d38518d63cb09484fd00d30da23b21d8a79" },
};

function hostTarget(): IpswTarget { return { platform: process.platform, arch: process.arch }; }
function platformKey(target: IpswTarget): string { return `${target.platform}-${target.arch}`; }
function executableName(target: IpswTarget): "ipsw" | "ipsw.exe" { return target.platform === "win32" ? "ipsw.exe" : "ipsw"; }
function bundledRoot(): string { return fileURLToPath(new URL("../tools/ipsw/", import.meta.url)); }
function cacheRoot(): string { return join(homedir(), ".beale", "tools", "ipsw", `v${IPSW_VERSION}`); }

async function executableExists(path: string): Promise<boolean> {
  try { await access(path, constants.X_OK); return true; }
  catch { return false; }
}

async function installationExists(directory: string, target: IpswTarget): Promise<boolean> {
  return await executableExists(join(directory, executableName(target)))
    && await access(join(directory, "LICENSE"), constants.F_OK).then(() => true, () => false);
}

async function archiveSha256(path: string): Promise<string> {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

/** Extract only the executable and its license after verifying a pinned archive. */
export async function installIpswArchive(input: {
  archivePath: string;
  expectedSha256: string;
  destinationDirectory: string;
  executable: "ipsw" | "ipsw.exe";
}): Promise<string> {
  const digest = await archiveSha256(input.archivePath);
  if (digest !== input.expectedSha256) throw new Error("The ipsw archive checksum does not match the pinned release.");
  await mkdir(input.destinationDirectory, { recursive: true, mode: 0o700 });
  const selected = new Set([input.executable, "LICENSE"]);
  if (input.archivePath.endsWith(".zip")) {
    const files = unzipSync(await readFile(input.archivePath), { filter: (file) => selected.has(file.name) });
    for (const name of selected) if (files[name]) {
      await writeFile(join(input.destinationDirectory, name), files[name], { mode: 0o600 });
    }
  } else if (input.archivePath.endsWith(".tar.gz")) {
    await extractTar({ file: input.archivePath, cwd: input.destinationDirectory, strict: true,
      filter: (path, entry) => selected.has(path.replace(/^\.\//u, ""))
        && ("type" in entry ? entry.type === "File" : entry.isFile()) });
  } else throw new Error("Unsupported ipsw archive format.");
  const executable = join(input.destinationDirectory, input.executable);
  await access(executable, constants.F_OK);
  await access(join(input.destinationDirectory, "LICENSE"), constants.F_OK);
  await chmod(executable, 0o755);
  return executable;
}

async function downloadArchive(asset: IpswAsset, destination: string, signal?: AbortSignal, deadline?: number): Promise<void> {
  const remaining = deadline === undefined ? DOWNLOAD_TIMEOUT_MS : Math.min(DOWNLOAD_TIMEOUT_MS, deadline - Date.now());
  if (remaining <= 0 || signal?.aborted) throw new Error("ipsw provisioning was aborted or timed out.");
  const timeout = AbortSignal.timeout(remaining);
  const combined = signal ? AbortSignal.any([signal, timeout]) : timeout;
  const url = `https://github.com/blacktop/ipsw/releases/download/v${IPSW_VERSION}/${asset.filename}`;
  const response = await fetch(url, { signal: combined, redirect: "follow" });
  if (!response.ok || !response.body) throw new Error(`ipsw release download failed with HTTP ${response.status}.`);
  const handle = await open(destination, "wx", 0o600);
  const reader = response.body.getReader();
  try {
    let bytes = 0;
    while (true) {
      const { value: chunk, done } = await reader.read();
      if (done) break;
      bytes += chunk.byteLength;
      if (bytes > MAX_ARCHIVE_BYTES) throw new Error("ipsw release archive exceeds the size limit.");
      await handle.writeFile(chunk);
    }
  } finally { reader.releaseLock(); await handle.close(); }
}

async function provisionIpsw(asset: IpswAsset, directory: string, targetPlatform: IpswTarget,
  signal?: AbortSignal, deadline?: number): Promise<void> {
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const target = join(directory, executableName(targetPlatform));
  const lock = join(directory, ".install.lock");
  const stopAt = Math.min(deadline ?? Date.now() + DOWNLOAD_TIMEOUT_MS, Date.now() + DOWNLOAD_TIMEOUT_MS);
  while (true) {
    if (await installationExists(directory, targetPlatform)) return;
    if (signal?.aborted || Date.now() >= stopAt) throw new Error("ipsw provisioning was aborted or timed out.");
    try { await mkdir(lock); break; }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      const age = await stat(lock).then((entry) => Date.now() - entry.mtimeMs).catch(() => 0);
      if (age > LOCK_STALE_MS) await rm(lock, { recursive: true, force: true });
      else await delay(200, undefined, { signal });
    }
  }
  const temporary = join(directory, `.install-${randomUUID()}`);
  try {
    if (await installationExists(directory, targetPlatform)) return;
    await mkdir(temporary, { mode: 0o700 });
    const archivePath = join(temporary, asset.filename);
    await downloadArchive(asset, archivePath, signal, stopAt);
    const extracted = join(temporary, "extracted");
    const binary = await installIpswArchive({ archivePath, expectedSha256: asset.sha256,
      destinationDirectory: extracted, executable: executableName(targetPlatform) });
    await copyFile(join(extracted, "LICENSE"), join(directory, "LICENSE"));
    await rm(target, { force: true });
    await rename(binary, target);
  } finally {
    await rm(temporary, { recursive: true, force: true });
    await rm(lock, { recursive: true, force: true });
  }
}

async function ensureIpswExecutableForTarget(target: IpswTarget,
  options: { signal?: AbortSignal; deadline?: number } = {}): Promise<string | null> {
  const key = platformKey(target);
  const asset = ASSETS[key];
  if (!asset) return null;
  const packagedDirectory = join(bundledRoot(), key);
  if (await installationExists(packagedDirectory, target)) return join(packagedDirectory, executableName(target));
  const directory = join(cacheRoot(), key);
  const cached = join(directory, executableName(target));
  if (await installationExists(directory, target)) return cached;
  await provisionIpsw(asset, directory, target, options.signal, options.deadline);
  return await installationExists(directory, target) ? cached : null;
}

/** Prefer the packaged binary; source checkouts provision a verified per-user copy on demand. */
export async function ensureIpswExecutable(options: { signal?: AbortSignal; deadline?: number } = {}): Promise<string | null> {
  return ensureIpswExecutableForTarget(hostTarget(), options);
}

/** Stage a verified local binary and license with the installed app-server. */
export async function stageIpswBinary(destinationRoot: string, target: IpswTarget = hostTarget()): Promise<string> {
  const executable = await ensureIpswExecutableForTarget(target);
  if (!executable) throw new Error(`No pinned ipsw release is available for ${platformKey(target)}.`);
  const directory = join(destinationRoot, platformKey(target));
  await mkdir(directory, { recursive: true });
  const staged = join(directory, executableName(target));
  await copyFile(executable, staged);
  await chmod(staged, 0o755);
  await copyFile(join(dirname(executable), "LICENSE"), join(directory, "LICENSE"));
  return staged;
}
