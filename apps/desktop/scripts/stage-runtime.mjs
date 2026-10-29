// Stages the app-server runtime that installed Beale builds ship in Electron's
// resources directory. electron-builder copies build/runtime/* there (see
// electron-builder.yml), and appServerInvocation.ts resolves it at runtime.
//
// The staged tree mirrors the monorepo layout, so every existing resolver that
// looks for packages/app-server-runtime/dist/cli.js, app-server/dist, and
// managed-plugins under one root keeps working unchanged:
//
//   build/runtime/
//     app-server/                       built @beale/app-server (dist, resources)
//     packages/app-server-runtime/      CLI entry used by the desktop protocol client
//     managed-plugins/                  bundled managed plugins
//     node_modules/                     hoisted production dependencies, shared
//
// Run after `pnpm build` (tsc -b), which produces each package's dist.
import { cpSync, existsSync, mkdirSync, renameSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const desktopRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repoRoot = resolve(desktopRoot, '..', '..');
const runtimeRoot = join(desktopRoot, 'build', 'runtime');

for (const entry of ['app-server/dist/headlessMain.js', 'packages/app-server-runtime/dist/cli.js', 'packages/research-agent/dist/index.js']) {
  if (!existsSync(join(repoRoot, entry))) {
    throw new Error(`Missing ${entry}. Run \`pnpm build\` at the repository root before staging the runtime.`);
  }
}
const { stageIpswBinary } = await import('../../../packages/research-agent/dist/ipsw-binary.js');
const { stagePortableBinary } = await import('../../../packages/research-agent/dist/portable-binaries.js');

rmSync(runtimeRoot, { recursive: true, force: true });
mkdirSync(runtimeRoot, { recursive: true });

// Use the lockfile-backed deploy implementation. Legacy deploy resolves again
// from the registry and can package dependencies absent from the reviewed lock.
// The hoisted linker keeps runtime imports in a portable node_modules tree.
const appServerRoot = join(runtimeRoot, 'app-server');
execFileSync('pnpm', [
  '--filter', '@beale/app-server',
  'deploy', '--prod', '--frozen-lockfile',
  '--config.inject-workspace-packages=true',
  '--config.node-linker=hoisted',
  appServerRoot
], { cwd: repoRoot, stdio: 'inherit', shell: process.platform === 'win32' });
if (!existsSync(join(appServerRoot, 'pnpm-lock.yaml'))) {
  throw new Error('The staged app-server has no deployment lockfile.');
}

// Share one node_modules at the root so both app-server/dist and
// packages/app-server-runtime/dist resolve dependencies by walking upward.
renameSync(join(appServerRoot, 'node_modules'), join(runtimeRoot, 'node_modules'));
rmSync(join(runtimeRoot, 'node_modules', '.bin'), { recursive: true, force: true });
rmSync(join(runtimeRoot, 'node_modules', '.pnpm'), { recursive: true, force: true });

// Match the architecture configured in electron-builder.yml, not the runner.
const packagedArch = process.platform === 'darwin' ? 'arm64'
  : process.platform === 'win32' ? 'x64' : process.arch;
await stageIpswBinary(join(runtimeRoot, 'node_modules', '@beale', 'research-agent', 'tools', 'ipsw'),
  { platform: process.platform, arch: packagedArch });
for (const name of ['jq', 'fd', 'opengrep']) {
  await stagePortableBinary(name, join(runtimeRoot, 'node_modules', '@beale', 'research-agent', 'tools', name),
    { platform: process.platform, arch: packagedArch });
}

cpSync(
  join(runtimeRoot, 'node_modules', '@beale', 'app-server-runtime'),
  join(runtimeRoot, 'packages', 'app-server-runtime'),
  { recursive: true }
);
cpSync(join(repoRoot, 'managed-plugins'), join(runtimeRoot, 'managed-plugins'), { recursive: true });

process.stdout.write(`Staged the app-server runtime at ${runtimeRoot}\n`);
