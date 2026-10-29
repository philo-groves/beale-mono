import { createHash, randomUUID } from "node:crypto";
import { constants, createReadStream } from "node:fs";
import { access, chmod, copyFile, mkdir, open, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { unzipSync } from "fflate";
import { x as extractTar } from "tar";

export type PortableBinaryName = "jq" | "fd" | "opengrep";
export interface PortableBinaryTarget { platform: NodeJS.Platform; arch: string; }

interface ReleaseAsset { filename: string; sha256: string; }
interface ReleaseSpec {
  version: string;
  releaseUrl: string;
  assets: Record<string, ReleaseAsset>;
  licenses: readonly string[];
  separateLicense?: { url: string; sha256: string; filename: string };
  maxDownloadBytes?: number;
}

// Asset digests are from the respective projects' GitHub release manifests.
const RELEASES: Record<PortableBinaryName, ReleaseSpec> = {
  jq: {
    version: "1.8.2",
    releaseUrl: "https://github.com/jqlang/jq/releases/download/jq-1.8.2",
    licenses: ["COPYING"],
    separateLicense: {
      url: "https://raw.githubusercontent.com/jqlang/jq/jq-1.8.2/COPYING",
      sha256: "ad2b4a266b2268939c1446979759706077421cf906a203aa188c6f396e8cfd74",
      filename: "COPYING",
    },
    assets: {
      "darwin-arm64": { filename: "jq-macos-arm64", sha256: "2d75340ba57a4b4b4c8708a21c2dc8e958a48aaa8bba13b27f77f6e4c0eca07e" },
      "darwin-x64": { filename: "jq-macos-amd64", sha256: "e94b266e3c26690550006abe63152b782280f4e14374accdf04cbde844f00bc0" },
      "linux-arm64": { filename: "jq-linux-arm64", sha256: "8b85c817833814ddca00a144c33705546355afccf0cf39b188f3cdb48b852309" },
      "linux-x64": { filename: "jq-linux-amd64", sha256: "b1c22172dd303f3be49e935aa56aa48a8b7a46e0bc838b4997d3bb451495870f" },
      "win32-arm64": { filename: "jq-windows-arm64.exe", sha256: "083b5377392bc57cf27052b6d20a2d927770683bca844632901ff38b4b7b0ac7" },
      "win32-x64": { filename: "jq-windows-amd64.exe", sha256: "a6fc67fedaf9128a3309a1e2ebb8b986aeccf70122ee46d2cb4849e423f0c627" },
    },
  },
  fd: {
    version: "10.5.0",
    releaseUrl: "https://github.com/sharkdp/fd/releases/download/v10.5.0",
    licenses: ["LICENSE-MIT", "LICENSE-APACHE"],
    assets: {
      "darwin-arm64": { filename: "fd-v10.5.0-aarch64-apple-darwin.tar.gz", sha256: "b67e1836c468e42e411984b56e52fa7abec08c2bd22c867398e7cc134aac5e12" },
      "darwin-x64": { filename: "fd-v10.5.0-x86_64-apple-darwin.tar.gz", sha256: "7e31028c62c6955877735d0406807aa484c2a5e6f86235a59e26c29c301da590" },
      "linux-arm64": { filename: "fd-v10.5.0-aarch64-unknown-linux-musl.tar.gz", sha256: "d76c4317f7d5dba69f8a2a15856c90c777e7f0dd4e85f0de8c76de6992c374d4" },
      "linux-x64": { filename: "fd-v10.5.0-x86_64-unknown-linux-musl.tar.gz", sha256: "761c72dc8e120d85b22292063be8a796e2eeb20eb3e4f38b8fa2343ccf3514a7" },
      "win32-arm64": { filename: "fd-v10.5.0-aarch64-pc-windows-msvc.zip", sha256: "a2bcddcfd259b05357a77bbc6cd671fdb30f63fd266a0e748305890a8c5ceaa6" },
      "win32-x64": { filename: "fd-v10.5.0-x86_64-pc-windows-msvc.zip", sha256: "a227701b8551c35a9931d9f6da75503cf86d88e182d71fb849a70864c5d57cd7" },
    },
  },
  opengrep: {
    version: "1.33.0",
    releaseUrl: "https://github.com/opengrep/opengrep/releases/download/v1.33.0",
    licenses: ["LICENSE"],
    separateLicense: {
      url: "https://raw.githubusercontent.com/opengrep/opengrep/v1.33.0/LICENSE",
      sha256: "20c17d8b8c48a600800dfd14f95d5cb9ff47066a9641ddeab48dc54aec96e331",
      filename: "LICENSE",
    },
    maxDownloadBytes: 64 * 1024 * 1024,
    assets: {
      "darwin-arm64": { filename: "opengrep_osx_arm64", sha256: "87d38a3ca198694a651e723b503e3ff8b0547429b4b9dfa2946f75fc43bb1962" },
      "darwin-x64": { filename: "opengrep_osx_x86", sha256: "838b74a056be35adfd1067ed7567e0d946b73066ba1b6be8fe5fe966b2142a7e" },
      "linux-arm64": { filename: "opengrep_manylinux_aarch64", sha256: "d58d332649c347678431647661d83f151aa81a0871648228e16a5767117dff92" },
      "linux-x64": { filename: "opengrep_manylinux_x86", sha256: "2545812755572e1355ea60f96288f82a26eebed7fc8b512430d49d5667214c24" },
      "win32-arm64": { filename: "opengrep_windows_x86.exe", sha256: "9a9e90f187f73149f4d141f78241d18282538ce94d555d7515ca9cd70e8f22e9" },
      "win32-x64": { filename: "opengrep_windows_x86.exe", sha256: "9a9e90f187f73149f4d141f78241d18282538ce94d555d7515ca9cd70e8f22e9" },
    },
  },
};

const MAX_DOWNLOAD_BYTES = 32 * 1024 * 1024;
const DOWNLOAD_TIMEOUT_MS = 120_000;
const LOCK_STALE_MS = 3 * 60_000;

function hostTarget(): PortableBinaryTarget { return { platform: process.platform, arch: process.arch }; }
function targetKey(target: PortableBinaryTarget): string { return `${target.platform}-${target.arch}`; }
function executableName(name: PortableBinaryName, target: PortableBinaryTarget): string {
  return target.platform === "win32" ? `${name}.exe` : name;
}
function bundledRoot(name: PortableBinaryName): string {
  return fileURLToPath(new URL(`../tools/${name}/`, import.meta.url));
}
function cacheRoot(name: PortableBinaryName): string {
  return join(homedir(), ".beale", "tools", name, `v${RELEASES[name].version}`);
}

async function sha256(path: string): Promise<string> {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

async function verifiedDownload(url: string, destination: string, digest: string,
  signal?: AbortSignal, deadline?: number, maxBytes = MAX_DOWNLOAD_BYTES): Promise<void> {
  const remaining = deadline === undefined ? DOWNLOAD_TIMEOUT_MS : Math.min(DOWNLOAD_TIMEOUT_MS, deadline - Date.now());
  if (remaining <= 0 || signal?.aborted) throw new Error("Portable tool provisioning was aborted or timed out.");
  const timeout = AbortSignal.timeout(remaining);
  const combined = signal ? AbortSignal.any([signal, timeout]) : timeout;
  const response = await fetch(url, { signal: combined, redirect: "follow" });
  if (!response.ok || !response.body) throw new Error(`Portable tool download failed with HTTP ${response.status}.`);
  const handle = await open(destination, "wx", 0o600);
  const reader = response.body.getReader();
  try {
    let bytes = 0;
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > maxBytes) throw new Error("Portable tool download exceeds the size limit.");
      await handle.writeFile(value);
    }
  } finally { reader.releaseLock(); await handle.close(); }
  if (await sha256(destination) !== digest) throw new Error("Portable tool download checksum does not match the pinned release.");
}

/** Extract a verified release asset, retaining only its executable and license files. */
export async function unpackPortableArtifact(input: {
  artifactPath: string;
  expectedSha256: string;
  destinationDirectory: string;
  executable: string;
  licenses: readonly string[];
  archivePrefix?: string;
}): Promise<void> {
  if (await sha256(input.artifactPath) !== input.expectedSha256) {
    throw new Error("Portable tool archive checksum does not match the pinned release.");
  }
  await mkdir(input.destinationDirectory, { recursive: true, mode: 0o700 });
  const names = [input.executable, ...input.licenses];
  if (!input.archivePrefix) {
    await copyFile(input.artifactPath, join(input.destinationDirectory, input.executable));
  } else {
    const entries = new Map(names.map((name) => [`${input.archivePrefix}/${name}`, name]));
    if (input.artifactPath.endsWith(".zip")) {
      const files = unzipSync(await readFile(input.artifactPath), { filter: (file) => entries.has(file.name) });
      for (const [path, name] of entries) if (files[path]) {
        await writeFile(join(input.destinationDirectory, name), files[path], { mode: 0o600 });
      }
    } else if (input.artifactPath.endsWith(".tar.gz")) {
      const extracted = join(input.destinationDirectory, "archive");
      await mkdir(extracted);
      await extractTar({ file: input.artifactPath, cwd: extracted, strict: true,
        filter: (path, entry) => entries.has(path.replace(/^\.\//u, ""))
          && ("type" in entry ? entry.type === "File" : entry.isFile()) });
      for (const [path, name] of entries) {
        await copyFile(join(extracted, path), join(input.destinationDirectory, name));
      }
      await rm(extracted, { recursive: true, force: true });
    } else throw new Error("Unsupported portable tool archive format.");
  }
  await access(join(input.destinationDirectory, input.executable), constants.F_OK);
  for (const license of input.licenses) await access(join(input.destinationDirectory, license), constants.F_OK);
  await chmod(join(input.destinationDirectory, input.executable), 0o755);
}

async function installationExists(name: PortableBinaryName, directory: string,
  target: PortableBinaryTarget): Promise<boolean> {
  try {
    await access(join(directory, executableName(name, target)), constants.X_OK);
    for (const license of RELEASES[name].licenses) await access(join(directory, license), constants.F_OK);
    return true;
  } catch { return false; }
}

async function provision(name: PortableBinaryName, target: PortableBinaryTarget, directory: string,
  asset: ReleaseAsset, signal?: AbortSignal, deadline?: number): Promise<void> {
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const lock = join(directory, ".install.lock");
  const stopAt = Math.min(deadline ?? Date.now() + DOWNLOAD_TIMEOUT_MS, Date.now() + DOWNLOAD_TIMEOUT_MS);
  while (true) {
    if (await installationExists(name, directory, target)) return;
    if (signal?.aborted || Date.now() >= stopAt) throw new Error("Portable tool provisioning was aborted or timed out.");
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
    if (await installationExists(name, directory, target)) return;
    await mkdir(temporary, { mode: 0o700 });
    const release = RELEASES[name];
    const artifactPath = join(temporary, asset.filename);
    await verifiedDownload(`${release.releaseUrl}/${asset.filename}`, artifactPath, asset.sha256, signal, stopAt,
      release.maxDownloadBytes);
    const extracted = join(temporary, "extracted");
    const executable = executableName(name, target);
    const prefix = asset.filename.replace(/(?:\.tar\.gz|\.zip)$/u, "");
    await unpackPortableArtifact({ artifactPath, expectedSha256: asset.sha256, destinationDirectory: extracted,
      executable, licenses: release.separateLicense ? [] : release.licenses,
      ...(asset.filename.endsWith(".zip") || asset.filename.endsWith(".tar.gz") ? { archivePrefix: prefix } : {}) });
    if (release.separateLicense) {
      await verifiedDownload(release.separateLicense.url, join(extracted, release.separateLicense.filename),
        release.separateLicense.sha256, signal, stopAt);
    }
    for (const license of release.licenses) {
      await copyFile(join(extracted, license), join(directory, license));
    }
    const installed = join(directory, executable);
    await rm(installed, { force: true });
    await rename(join(extracted, executable), installed);
  } finally {
    await rm(temporary, { recursive: true, force: true });
    await rm(lock, { recursive: true, force: true });
  }
}

async function ensureForTarget(name: PortableBinaryName, target: PortableBinaryTarget,
  options: { signal?: AbortSignal; deadline?: number } = {}): Promise<string | null> {
  const key = targetKey(target);
  const asset = RELEASES[name].assets[key];
  if (!asset) return null;
  const packaged = join(bundledRoot(name), key);
  if (await installationExists(name, packaged, target)) return join(packaged, executableName(name, target));
  const cached = join(cacheRoot(name), key);
  if (await installationExists(name, cached, target)) return join(cached, executableName(name, target));
  await provision(name, target, cached, asset, options.signal, options.deadline);
  return await installationExists(name, cached, target) ? join(cached, executableName(name, target)) : null;
}

export async function ensurePortableBinary(name: PortableBinaryName,
  options: { signal?: AbortSignal; deadline?: number } = {}): Promise<string | null> {
  return ensureForTarget(name, hostTarget(), options);
}

export async function stagePortableBinary(name: PortableBinaryName, destinationRoot: string,
  target: PortableBinaryTarget = hostTarget()): Promise<string> {
  const executable = await ensureForTarget(name, target);
  if (!executable) throw new Error(`No pinned ${name} release is available for ${targetKey(target)}.`);
  const destination = join(destinationRoot, targetKey(target));
  await mkdir(destination, { recursive: true });
  const staged = join(destination, executableName(name, target));
  await copyFile(executable, staged);
  await chmod(staged, 0o755);
  for (const license of RELEASES[name].licenses) {
    const stagedLicense = join(destination, license);
    await copyFile(join(dirname(executable), license), stagedLicense);
    await chmod(stagedLicense, 0o644);
  }
  return staged;
}
