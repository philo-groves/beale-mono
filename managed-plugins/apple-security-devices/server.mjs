import { createHash, randomUUID } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { AsyncLocalStorage } from 'node:async_hooks';
import {
  chmodSync,
  closeSync,
  createReadStream,
  createWriteStream,
  existsSync,
  linkSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  realpathSync,
  renameSync,
  statSync,
  unlinkSync,
  writeFileSync
} from 'node:fs';
import net from 'node:net';
import { basename, dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { homedir, tmpdir } from 'node:os';
import { Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';

const MAX_COMMAND_OUTPUT_BYTES = 512 * 1024;
const MAX_CONSOLE_OUTPUT_BYTES = 128 * 1024;
const MAX_LOG_BYTES = 128 * 1024;
const MAX_TART_COPY_BYTES = 4 * 1024 * 1024 * 1024;
const DEFAULT_TART_COPY_BYTES = MAX_TART_COPY_BYTES;
const TART_GUEST_DESCRIPTOR_RECOVERY_THRESHOLD = 96;
const TART_GUEST_DESCRIPTOR_PROBE_INTERVAL = 12;
const TART_GUEST_EXEC_HELPER_PATH = '/tmp/.beale-tart-exec-v3';
const TART_GUEST_EXEC_HELPER_SOURCE = String.raw`
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static long descriptor_limit(void) {
  long limit = sysconf(_SC_OPEN_MAX);
  if (limit < 0 || limit > 1048576) return 1048576;
  return limit;
}

static int descriptor_count(void) {
  int count = 0;
  long limit = descriptor_limit();
  for (int fd = 0; fd < limit; fd++) {
    errno = 0;
    if (fcntl(fd, F_GETFD) != -1 || errno != EBADF) count++;
  }
  return count;
}

int main(int argc, char **argv) {
  if (argc == 2 && strcmp(argv[1], "--beale-probe") == 0) {
    printf("%d\n", descriptor_count());
    return 0;
  }
  if (argc < 2) return 64;
  long limit = descriptor_limit();
  for (int fd = (int)limit - 1; fd >= 3; fd--) close(fd);
  execvp(argv[1], &argv[1]);
  fprintf(stderr, "beale-tart-exec: execvp failed: %s\n", strerror(errno));
  return 127;
}
`.trimStart();
const SIMULATOR_PATTERN = /(?:\bios simulator\b|\bsimulator\b|\bsimctl\b|\biphonesimulator\b)/iu;
const PLUGIN_DATA = resolve(process.env.PLUGIN_DATA || join(tmpdir(), 'beale-apple-security-devices'));
const RUNS_ROOT = join(PLUGIN_DATA, 'darwin-vm-runs');
const TART_LOG_ROOT = join(PLUGIN_DATA, 'tart-logs');
const deviceRefs = new Map();
const activeDarwinRuns = new Map();
const tartGuestTransports = new Map();
const tartGuestExecHelpers = new Map();
const tartGuestOperationsSinceDescriptorProbe = new Map();
const tartGuestRestartRequired = new Map();
const tartVmOperationTails = new Map();
const activeRequestControllers = new Map();
const requestSignalStorage = new AsyncLocalStorage();
const deviceRefSalt = randomUUID();
let tartHostExecHelperPromise = null;
let inputBuffer = '';

mkdirSync(PLUGIN_DATA, { recursive: true, mode: 0o700 });
mkdirSync(RUNS_ROOT, { recursive: true, mode: 0o700 });
mkdirSync(TART_LOG_ROOT, { recursive: true, mode: 0o700 });

const READ_ANNOTATION = toolAnnotation(['inspect'], 'read', ['apple-security-devices:observe'], 'never');
const WRITE_ANNOTATION = toolAnnotation(['experiment'], 'write', ['apple-security-devices:mutate'], 'always');
const TART_OPERATION_ANNOTATION = toolAnnotation(['experiment'], 'write', ['apple-security-devices:mutate'], 'never');

const TOOLS = [
  {
    name: 'environment_status',
    description: 'Report whether Tart and Xcode CoreDevice tooling are available without exposing host paths, devices, or identifiers.',
    inputSchema: objectSchema({}),
    annotations: READ_ANNOTATION
  },
  {
    name: 'list_tart_vms',
    description: 'List local Tart macOS virtual machines and their current state.',
    inputSchema: objectSchema({}),
    annotations: READ_ANNOTATION
  },
  {
    name: 'tart_vm_ip',
    description: 'Resolve the current IP address of one explicitly named running Tart macOS virtual machine. Concurrent guests do not affect name-bound resolution.',
    inputSchema: objectSchema({ vmName: stringField(128), waitSeconds: integerField(0, 60, 10) }, ['vmName']),
    annotations: READ_ANNOTATION
  },
  {
    name: 'inspect_tart_vm',
    description: 'Inspect one Tart VM and, when running, probe its guest agent plus fixed macOS build, architecture, and SIP baseline without SSH or an IP address.',
    inputSchema: objectSchema({ vmName: stringField(128), timeoutSeconds: integerField(1, 30, 10) }, ['vmName']),
    annotations: READ_ANNOTATION
  },
  {
    name: 'start_tart_vm',
    description: 'Start one existing Tart macOS VM headlessly and wait for its guest agent. Concurrent name-bound guests are allowed by default; request exclusivity only when an experiment requires it.',
    inputSchema: objectSchema({
      vmName: stringField(128),
      requireExclusive: { type: 'boolean', default: false },
      networkMode: { type: 'string', enum: ['shared', 'host-only'], default: 'shared' },
      waitSeconds: integerField(0, 90, 45)
    }, ['vmName']),
    annotations: TART_OPERATION_ANNOTATION
  },
  {
    name: 'stop_tart_vm',
    description: 'Stop a running Tart macOS VM.',
    inputSchema: objectSchema({ vmName: stringField(128), timeoutSeconds: integerField(1, 120, 30) }, ['vmName']),
    annotations: TART_OPERATION_ANNOTATION
  },
  {
    name: 'exec_tart_vm',
    description: 'Execute one bounded argument-vector command in a named Tart VM through a native descriptor-sanitizing Guest Agent child or the configured private SSH fallback without invoking a host shell. Recycles Guest Agent descriptor pressure before running the requested command.',
    inputSchema: objectSchema({
      vmName: stringField(128),
      argv: { type: 'array', minItems: 1, maxItems: 128, items: stringField(4096) },
      timeoutSeconds: integerField(1, 300, 60)
    }, ['vmName', 'argv']),
    annotations: TART_OPERATION_ANNOTATION
  },
  {
    name: 'copy_to_tart_vm',
    description: 'Stream one regular file from an absolute host path to an absolute path in a named running Tart VM. Transfers up to 4 GiB by default; maxBytes may impose a tighter per-call bound.',
    inputSchema: objectSchema({
      vmName: stringField(128),
      localPath: stringField(4096),
      guestPath: stringField(4096),
      overwrite: { type: 'boolean', default: false },
      preserveMode: { type: 'boolean', default: true },
      maxBytes: integerField(1, MAX_TART_COPY_BYTES, DEFAULT_TART_COPY_BYTES),
      timeoutSeconds: integerField(1, 3600, 900)
    }, ['vmName', 'localPath', 'guestPath']),
    annotations: TART_OPERATION_ANNOTATION
  },
  {
    name: 'copy_from_tart_vm',
    description: 'Stream one regular file from an absolute path in a named running Tart VM to an absolute host path. Transfers up to 4 GiB by default; maxBytes may impose a tighter per-call bound.',
    inputSchema: objectSchema({
      vmName: stringField(128),
      guestPath: stringField(4096),
      localPath: stringField(4096),
      overwrite: { type: 'boolean', default: false },
      preserveMode: { type: 'boolean', default: true },
      maxBytes: integerField(1, MAX_TART_COPY_BYTES, DEFAULT_TART_COPY_BYTES),
      timeoutSeconds: integerField(1, 3600, 900)
    }, ['vmName', 'guestPath', 'localPath']),
    annotations: TART_OPERATION_ANNOTATION
  },
  {
    name: 'list_physical_iphones',
    description: 'List paired physical iPhones visible to Xcode CoreDevice using opaque, process-local device references. Simulator devices are never returned.',
    inputSchema: objectSchema({ timeoutSeconds: integerField(5, 60, 15) }),
    annotations: READ_ANNOTATION
  },
  {
    name: 'describe_physical_iphone',
    description: 'Read bounded product, OS, connection, and developer-state details for one previously listed physical iPhone without returning durable identifiers.',
    inputSchema: objectSchema({ deviceRef: deviceRefField(), timeoutSeconds: integerField(5, 60, 15) }, ['deviceRef']),
    annotations: READ_ANNOTATION
  },
  {
    name: 'install_physical_iphone_app',
    description: 'Install an operator-built .app bundle on one previously listed physical iPhone through Xcode CoreDevice.',
    inputSchema: objectSchema({ deviceRef: deviceRefField(), appPath: stringField(4096), timeoutSeconds: integerField(5, 300, 120) }, ['deviceRef', 'appPath']),
    annotations: WRITE_ANNOTATION
  },
  {
    name: 'launch_physical_iphone_app',
    description: 'Launch an installed bundle on one previously listed physical iPhone, optionally suspended for debugger attachment.',
    inputSchema: objectSchema({
      deviceRef: deviceRefField(),
      bundleIdentifier: stringField(255),
      arguments: { type: 'array', maxItems: 128, items: stringField(4096) },
      environment: { type: 'object', maxProperties: 64, additionalProperties: { type: 'string', maxLength: 4096 } },
      startStopped: { type: 'boolean', default: false },
      terminateExisting: { type: 'boolean', default: false },
      timeoutSeconds: integerField(5, 300, 60)
    }, ['deviceRef', 'bundleIdentifier']),
    annotations: WRITE_ANNOTATION
  },
  {
    name: 'inspect_darwin_vm',
    description: 'Inspect the configured Darwin VM checkout, or an explicit checkoutRoot, for its built QEMU and required, optionally hashed firmware artifacts.',
    inputSchema: objectSchema({ checkoutRoot: stringField(4096), hashArtifacts: { type: 'boolean', default: false } }),
    annotations: READ_ANNOTATION
  },
  {
    name: 'list_darwin_vm_runs',
    description: 'List darwin-vm runs started by this plugin without exposing host paths.',
    inputSchema: objectSchema({}),
    annotations: READ_ANNOTATION
  },
  {
    name: 'read_darwin_vm_log',
    description: 'Read a bounded tail of the serial log for a darwin-vm run started by this plugin.',
    inputSchema: objectSchema({ runId: runIdField(), maxBytes: integerField(1024, MAX_LOG_BYTES, 32768) }, ['runId']),
    annotations: READ_ANNOTATION
  },
  {
    name: 'start_darwin_vm',
    description: 'Start the configured Darwin VM checkout (or explicit checkoutRoot) through built QEMU with no emulated network device, host share, graphics, or QEMU monitor.',
    inputSchema: objectSchema({
      checkoutRoot: stringField(4096),
      memoryMiB: integerField(2048, 32768, 8192),
      bootArguments: stringField(2048)
    }),
    annotations: WRITE_ANNOTATION
  },
  {
    name: 'stop_darwin_vm',
    description: 'Stop a darwin-vm process previously started by this plugin.',
    inputSchema: objectSchema({ runId: runIdField() }, ['runId']),
    annotations: WRITE_ANNOTATION
  },
  {
    name: 'run_darwin_vm_console_command',
    description: 'Send one newline-terminated command to the serial console of a darwin-vm run and return bounded console output.',
    inputSchema: objectSchema({
      runId: runIdField(),
      command: stringField(1024),
      readMilliseconds: integerField(100, 5000, 1000)
    }, ['runId', 'command']),
    annotations: WRITE_ANNOTATION
  }
];

process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => {
  inputBuffer += chunk;
  drainMessages();
});
process.stdin.on('error', (error) => console.error(errorMessage(error)));

function drainMessages() {
  while (true) {
    const newlineIndex = inputBuffer.indexOf('\n');
    if (newlineIndex < 0) return;
    const line = inputBuffer.slice(0, newlineIndex).replace(/\r$/u, '').trim();
    inputBuffer = inputBuffer.slice(newlineIndex + 1);
    if (line) handleMessage(line);
  }
}

function handleMessage(body) {
  let message;
  try {
    message = JSON.parse(body);
  } catch {
    sendError(null, -32700, 'Invalid JSON-RPC payload.');
    return;
  }
  if (message.method === 'notifications/cancelled') {
    const requestId = message.params?.requestId;
    if (typeof requestId === 'number' || typeof requestId === 'string') {
      activeRequestControllers.get(requestId)?.abort(
        new Error(typeof message.params?.reason === 'string' ? message.params.reason : 'MCP request cancelled.')
      );
    }
    return;
  }
  if (message.method?.startsWith('notifications/')) return;
  Promise.resolve(dispatch(message)).catch((error) => sendError(message.id ?? null, -32603, publicError(error)));
}

async function dispatch(message) {
  const { id, method, params } = message;
  if (method === 'initialize') {
    sendResult(id, {
      protocolVersion: params?.protocolVersion ?? '2025-11-25',
      capabilities: { tools: {} },
      serverInfo: { name: 'apple-security-devices', version: '0.1.0' }
    });
    return;
  }
  if (method === 'ping') {
    sendResult(id, {});
    return;
  }
  if (method === 'tools/list') {
    sendResult(id, { tools: TOOLS });
    return;
  }
  if (method !== 'tools/call') {
    sendError(id ?? null, -32601, `Unsupported method: ${String(method)}`);
    return;
  }
  const name = typeof params?.name === 'string' ? params.name : '';
  const args = isRecord(params?.arguments) ? params.arguments : {};
  const tool = TOOLS.find((candidate) => candidate.name === name);
  if (!tool) {
    sendToolError(id, `Unknown Apple security devices tool: ${name}`);
    return;
  }
  const controller = new AbortController();
  activeRequestControllers.set(id, controller);
  try {
    assertNoSimulator(args);
    const result = await requestSignalStorage.run(
      controller.signal,
      () => callTool(name, args)
    );
    if (!controller.signal.aborted) sendResult(id, textResult(result));
  } catch (error) {
    if (!controller.signal.aborted) sendToolError(id, publicError(error, args));
  } finally {
    if (activeRequestControllers.get(id) === controller) activeRequestControllers.delete(id);
  }
}

async function callTool(name, args) {
  if (['inspect_tart_vm', 'start_tart_vm', 'stop_tart_vm', 'exec_tart_vm', 'copy_to_tart_vm', 'copy_from_tart_vm'].includes(name)) {
    const vmName = safeVmName(args.vmName);
    return withTartVmOperation(vmName, () => callToolUnlocked(name, args));
  }
  return callToolUnlocked(name, args);
}

async function callToolUnlocked(name, args) {
  if (name === 'environment_status') return environmentStatus();
  if (name === 'list_tart_vms') return listTartVms();
  if (name === 'tart_vm_ip') return tartVmIp(args);
  if (name === 'inspect_tart_vm') return inspectTartVm(args);
  if (name === 'start_tart_vm') return startTartVm(args);
  if (name === 'stop_tart_vm') return stopTartVm(args);
  if (name === 'exec_tart_vm') return execTartVm(args);
  if (name === 'copy_to_tart_vm') return copyToTartVm(args);
  if (name === 'copy_from_tart_vm') return copyFromTartVm(args);
  if (name === 'list_physical_iphones') return listPhysicalIphones(args);
  if (name === 'describe_physical_iphone') return describePhysicalIphone(args);
  if (name === 'install_physical_iphone_app') return installPhysicalIphoneApp(args);
  if (name === 'launch_physical_iphone_app') return launchPhysicalIphoneApp(args);
  if (name === 'inspect_darwin_vm') return inspectDarwinVm(args);
  if (name === 'list_darwin_vm_runs') return listDarwinVmRuns();
  if (name === 'read_darwin_vm_log') return readDarwinVmLog(args);
  if (name === 'start_darwin_vm') return startDarwinVm(args);
  if (name === 'stop_darwin_vm') return stopDarwinVm(args);
  if (name === 'run_darwin_vm_console_command') return runDarwinVmConsoleCommand(args);
  throw new Error(`Unsupported Apple security devices tool: ${name}`);
}

async function withTartVmOperation(vmName, operation) {
  const previous = tartVmOperationTails.get(vmName) ?? Promise.resolve();
  const current = previous.catch(() => undefined).then(() => {
    const signal = requestSignalStorage.getStore();
    if (signal?.aborted) throw signal.reason ?? new Error('MCP request cancelled.');
    return operation();
  });
  tartVmOperationTails.set(vmName, current);
  try {
    return await current;
  } finally {
    if (tartVmOperationTails.get(vmName) === current) tartVmOperationTails.delete(vmName);
  }
}

async function environmentStatus() {
  const tart = await commandAvailability(tartCommand(), ['--version']);
  const transportPolicy = configuredTartTransportPolicy();
  const hostConfig = readAppleSecurityHostConfig();
  const developerDir = findDeveloperDir();
  const coreDevice = developerDir
    ? await commandAvailability(xcrunCommand(), ['devicectl', '--version'], { DEVELOPER_DIR: developerDir })
    : { available: false, detail: 'Xcode developer directory not found.' };
  return {
    hostPlatform: process.platform,
    tart: {
      ...tart,
      transportPolicy,
      hostCommandRunnerConfigured: transportPolicy === 'guest-agent-or-ssh'
        && Boolean(process.env.APPLE_SECURITY_COMMAND_RUNNER || hostConfig.commandRunner)
    },
    physicalIphone: {
      available: process.platform === 'darwin' && coreDevice.available,
      detail: coreDevice.detail
    },
    darwinVm: await darwinVmEnvironmentStatus(),
    iosSimulatorSupported: false
  };
}

async function listTartVms() {
  assertDarwinHost('Tart');
  return {
    vms: await readTartVms(),
    note: 'Tart guests are macOS research environments, not physical-iPhone substitutes.'
  };
}

async function readTartVms() {
  const result = await runCommand(tartCommand(), ['list', '--format', 'json'], { timeoutMs: 15_000 });
  const parsed = parseJson(result.stdout, 'Tart list output');
  if (!Array.isArray(parsed)) throw new Error('Tart list output was not an array.');
  return parsed.slice(0, 200).flatMap((entry) => {
      if (!isRecord(entry)) return [];
      const name = firstString(entry, ['Name', 'name']);
      if (!name) return [];
      return [{
        name,
        running: firstBoolean(entry, ['Running', 'running']) ?? false,
        state: firstString(entry, ['State', 'state']) ?? 'unknown',
        diskGiB: firstNumber(entry, ['Disk', 'disk']),
        sizeGiB: firstNumber(entry, ['Size', 'size']),
        source: firstString(entry, ['Source', 'source'])
      }];
    });
}

async function tartVmIp(args) {
  assertDarwinHost('Tart');
  const vmName = safeVmName(args.vmName);
  const vms = await readTartVms();
  const vm = requireKnownTartVm(vms, vmName);
  if (!vm.running) throw new Error(`Cannot resolve an IP address because Tart VM ${vmName} is not running.`);
  const waitSeconds = boundedInteger(args.waitSeconds, 0, 60, 10);
  const result = await runCommand(tartCommand(), ['ip', vmName, '--wait', String(waitSeconds)], {
    timeoutMs: (waitSeconds + 5) * 1000
  });
  const address = result.stdout.trim();
  if (!address || address.length > 128) throw new Error('Tart did not return a bounded VM address.');
  return {
    vmName,
    address,
    concurrentRunningVms: vms.filter((candidate) => candidate.running && candidate.name !== vmName).map((candidate) => candidate.name)
  };
}

async function inspectTartVm(args) {
  assertDarwinHost('Tart');
  const vmName = safeVmName(args.vmName);
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 1, 30, 10);
  const vms = await readTartVms();
  const vm = requireKnownTartVm(vms, vmName);
  return {
    vm,
    concurrentRunningVms: vms.filter((candidate) => candidate.running && candidate.name !== vmName).map((candidate) => candidate.name),
    guest: vm.running ? await tartGuestBaseline(vmName, timeoutSeconds) : { ready: false, detail: 'VM is stopped.' },
    recommendedTransport: configuredTartTransportPolicy() === 'guest-agent-only'
      ? 'Use exec_tart_vm and the Tart copy tools. This host requires Tart Guest Agent and will not fall back to SSH or a host command runner.'
      : 'Use exec_tart_vm and the Tart copy tools; the plugin selects Tart Guest Agent or its configured bounded SSH fallback without exposing connection details.'
  };
}

async function startTartVm(args) {
  assertDarwinHost('Tart');
  const vmName = safeVmName(args.vmName);
  const vms = await readTartVms();
  const vm = requireKnownTartVm(vms, vmName);
  const otherRunningVms = vms.filter((candidate) => candidate.running && candidate.name !== vmName).map((candidate) => candidate.name);
  const waitSeconds = boundedInteger(args.waitSeconds, 0, 90, 45);
  if (vm.running) {
    return {
      started: false,
      alreadyRunning: true,
      vmName,
      guest: await waitForTartGuest(vmName, waitSeconds),
      concurrentRunningVms: otherRunningVms
    };
  }
  if (args.requireExclusive === true && otherRunningVms.length > 0) {
    throw new Error(`Exclusive start requested for ${vmName}, but another Tart VM is running (${otherRunningVms.join(', ')}). Retry without requireExclusive or stop the other VM when isolation is required.`);
  }
  tartGuestTransports.delete(vmName);
  tartGuestExecHelpers.delete(vmName);
  tartGuestOperationsSinceDescriptorProbe.delete(vmName);
  tartGuestRestartRequired.delete(vmName);
  const networkMode = tartNetworkMode(args.networkMode);
  const logPath = join(TART_LOG_ROOT, `${safeFilename(vmName)}-${Date.now()}.log`);
  const tartArgs = [
    'run', vmName,
    '--no-graphics',
    '--no-audio',
    '--no-clipboard'
  ];
  if (networkMode === 'host-only') tartArgs.push('--net-host');
  await spawnDetached(tartCommand(), tartArgs, { cwd: PLUGIN_DATA, logPath, startupGraceMs: 750 });
  const guest = await waitForTartGuest(vmName, waitSeconds);
  const runningAfterStart = waitSeconds === 0
    ? true
    : (await readTartVms()).find((candidate) => candidate.name === vmName)?.running === true;
  if (!runningAfterStart) {
    const diagnostic = firstLines(readTail(logPath, 4096), 4) || guest.detail;
    throw new Error(`Tart VM ${vmName} exited during startup: ${diagnostic}`);
  }
  return {
    started: true,
    vmName,
    guest,
    concurrentRunningVms: otherRunningVms,
    posture: {
      graphics: false,
      audio: false,
      clipboard: false,
      network: networkMode === 'host-only' ? 'host-only' : 'shared-nat',
      hostShares: false
    }
  };
}

async function stopTartVm(args) {
  assertDarwinHost('Tart');
  const vmName = safeVmName(args.vmName);
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 1, 120, 30);
  await runCommand(tartCommand(), ['stop', vmName, '--timeout', String(timeoutSeconds)], {
    timeoutMs: (timeoutSeconds + 5) * 1000
  });
  tartGuestTransports.delete(vmName);
  tartGuestExecHelpers.delete(vmName);
  tartGuestOperationsSinceDescriptorProbe.delete(vmName);
  tartGuestRestartRequired.delete(vmName);
  return { stopped: true, vmName };
}

async function execTartVm(args) {
  assertDarwinHost('Tart');
  const vmName = safeVmName(args.vmName);
  const argv = safeArgv(args.argv);
  if (argv[0].startsWith('-')) throw new Error('The guest executable must not begin with a hyphen.');
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 1, 300, 60);
  const transport = await resolveTartGuestTransport(vmName, Math.min(timeoutSeconds, 10));
  const result = await runTartGuestCommand(vmName, argv, timeoutSeconds, transport);
  return { ...commandResult(result), transport };
}

async function copyToTartVm(args) {
  assertDarwinHost('Tart');
  const vmName = safeVmName(args.vmName);
  await requireRunningTartVm(vmName);
  const localPath = canonicalFile(args.localPath, 'localPath');
  const guestPath = safeGuestFilePath(args.guestPath, 'guestPath');
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 1, 3600, 900);
  const maxBytes = boundedInteger(args.maxBytes, 1, MAX_TART_COPY_BYTES, DEFAULT_TART_COPY_BYTES);
  const localStat = statSync(localPath);
  if (localStat.size > maxBytes) throw new Error(`localPath exceeds the ${maxBytes}-byte transfer limit.`);
  const transport = await resolveTartGuestTransport(vmName, Math.min(timeoutSeconds, 10));
  const guestPathExists = await tartGuestPathExists(vmName, guestPath, timeoutSeconds, transport);
  if (guestPathExists) {
    if (args.overwrite !== true) throw new Error('guestPath already exists; set overwrite=true to replace it.');
    await requireTartGuestRegularFile(vmName, guestPath, timeoutSeconds, transport, 'guestPath');
  }
  const temporaryPath = temporaryGuestPath(guestPath);
  const expectedSha256 = await sha256File(localPath);
  let runnerUploadPath = null;
  try {
    const commandRunner = transport === 'ssh' ? configuredHostCommandRunner() : null;
    if (commandRunner) {
      runnerUploadPath = `/tmp/.beale-runner-upload-${randomUUID()}`;
      const invocation = await tartScpUploadInvocation(
        vmName,
        localPath,
        runnerUploadPath,
        timeoutSeconds,
        commandRunner
      );
      await runCommand(invocation.command, invocation.args, { timeoutMs: timeoutSeconds * 1000 });
      await runTartGuestCommand(
        vmName,
        ['/bin/dd', `if=${runnerUploadPath}`, `of=${temporaryPath}`, 'bs=1048576'],
        timeoutSeconds,
        transport
      );
      await removeTartGuestTemporaryFile(vmName, runnerUploadPath, timeoutSeconds, transport);
      runnerUploadPath = null;
    } else {
      const invocation = await tartGuestCommandInvocation(
        vmName,
        ['/bin/dd', `of=${temporaryPath}`, 'bs=1048576'],
        timeoutSeconds,
        transport,
        true
      );
      await runCommandWithFileInput(invocation.command, invocation.args, localPath, {
        timeoutMs: timeoutSeconds * 1000,
        maxBytes
      });
    }
    if (args.preserveMode !== false) {
      await runTartGuestCommand(vmName, ['/bin/chmod', (localStat.mode & 0o777).toString(8), temporaryPath], timeoutSeconds, transport);
    }
    const staged = await tartGuestFileMetadata(vmName, temporaryPath, timeoutSeconds, transport);
    if (staged.bytes !== localStat.size || staged.sha256 !== expectedSha256) {
      throw new Error('Guest staging verification did not match the host source file.');
    }
    if (args.overwrite !== true && await tartGuestPathExists(vmName, guestPath, timeoutSeconds, transport)) {
      throw new Error('guestPath appeared during transfer; refusing to overwrite it.');
    }
    await runTartGuestCommand(
      vmName,
      ['/bin/mv', args.overwrite === true ? '-f' : '-n', temporaryPath, guestPath],
      timeoutSeconds,
      transport
    );
    if (args.overwrite !== true && await tartGuestPathExists(vmName, temporaryPath, timeoutSeconds, transport)) {
      throw new Error('guestPath appeared while committing the transfer; it was not overwritten.');
    }
    const installed = await tartGuestFileMetadata(vmName, guestPath, timeoutSeconds, transport);
    if (installed.bytes !== localStat.size || installed.sha256 !== expectedSha256) {
      throw new Error('Guest destination verification did not match the host source file.');
    }
    return {
      copied: true,
      direction: 'host-to-guest',
      vmName,
      localFile: basename(localPath),
      guestPath,
      bytes: installed.bytes,
      sha256: installed.sha256,
      mode: fileModeText(installed.mode),
      transport
    };
  } catch (error) {
    if (runnerUploadPath) {
      await removeTartGuestTemporaryFile(vmName, runnerUploadPath, timeoutSeconds, transport);
    }
    await removeTartGuestTemporaryFile(vmName, temporaryPath, timeoutSeconds, transport);
    throw error;
  }
}

async function copyFromTartVm(args) {
  assertDarwinHost('Tart');
  const vmName = safeVmName(args.vmName);
  await requireRunningTartVm(vmName);
  const guestPath = safeGuestFilePath(args.guestPath, 'guestPath');
  const localPath = destinationFile(args.localPath, 'localPath');
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 1, 3600, 900);
  const maxBytes = boundedInteger(args.maxBytes, 1, MAX_TART_COPY_BYTES, DEFAULT_TART_COPY_BYTES);
  if (existsSync(localPath) && args.overwrite !== true) {
    throw new Error('localPath already exists; set overwrite=true to replace it.');
  }
  const transport = await resolveTartGuestTransport(vmName, Math.min(timeoutSeconds, 10));
  await requireTartGuestRegularFile(vmName, guestPath, timeoutSeconds, transport, 'guestPath');
  const guest = await tartGuestFileMetadata(vmName, guestPath, timeoutSeconds, transport);
  if (guest.bytes > maxBytes) throw new Error(`guestPath exceeds the ${maxBytes}-byte transfer limit.`);
  const temporaryPath = join(dirname(localPath), `.${safeFilename(basename(localPath))}.beale-download-${randomUUID()}`);
  try {
    const invocation = await tartGuestCommandInvocation(vmName, ['/bin/cat', guestPath], timeoutSeconds, transport, false);
    await runCommandToFile(invocation.command, invocation.args, temporaryPath, {
      timeoutMs: timeoutSeconds * 1000,
      maxBytes
    });
    const localStat = statSync(temporaryPath);
    const localSha256 = await sha256File(temporaryPath);
    if (localStat.size !== guest.bytes || localSha256 !== guest.sha256) {
      throw new Error('Host staging verification did not match the guest source file.');
    }
    if (args.preserveMode !== false) chmodSync(temporaryPath, guest.mode & 0o777);
    if (args.overwrite === true) renameSync(temporaryPath, localPath);
    else {
      linkSync(temporaryPath, localPath);
      unlinkSync(temporaryPath);
    }
    return {
      copied: true,
      direction: 'guest-to-host',
      vmName,
      guestPath,
      localFile: basename(localPath),
      bytes: guest.bytes,
      sha256: guest.sha256,
      mode: args.preserveMode === false ? null : fileModeText(guest.mode),
      transport
    };
  } catch (error) {
    removeFileIfPresent(temporaryPath);
    throw error;
  }
}

async function requireRunningTartVm(vmName) {
  const vm = requireKnownTartVm(await readTartVms(), vmName);
  if (!vm.running) throw new Error(`Cannot transfer a file because Tart VM ${vmName} is not running.`);
  return vm;
}

async function tartGuestPathExists(vmName, path, timeoutSeconds, transport) {
  try {
    await runTartGuestCommand(vmName, ['/bin/test', '-e', path], timeoutSeconds, transport);
    return true;
  } catch (error) {
    if (error instanceof CommandError && error.result.code === 1) return false;
    throw error;
  }
}

async function requireTartGuestRegularFile(vmName, path, timeoutSeconds, transport, field) {
  try {
    await runTartGuestCommand(vmName, ['/bin/test', '-f', path], timeoutSeconds, transport);
  } catch (error) {
    if (error instanceof CommandError && error.result.code === 1) throw new Error(`${field} must identify a regular guest file.`);
    throw error;
  }
}

async function tartGuestFileMetadata(vmName, path, timeoutSeconds, transport) {
  const sizeResult = await runTartGuestCommand(vmName, ['/usr/bin/stat', '-f', '%z', path], timeoutSeconds, transport);
  const modeResult = await runTartGuestCommand(vmName, ['/usr/bin/stat', '-f', '%Lp', path], timeoutSeconds, transport);
  const hashResult = await runTartGuestCommand(vmName, ['/usr/bin/shasum', '-a', '256', path], timeoutSeconds, transport);
  const bytes = Number.parseInt(firstLine(sizeResult.stdout), 10);
  const modeText = firstLine(modeResult.stdout);
  const sha256 = firstLine(hashResult.stdout).split(/\s+/u)[0]?.toLowerCase() ?? '';
  if (!Number.isSafeInteger(bytes) || bytes < 0) throw new Error('Guest file size was invalid.');
  if (!/^[0-7]{3,4}$/u.test(modeText)) throw new Error('Guest file mode was invalid.');
  if (!/^[0-9a-f]{64}$/u.test(sha256)) throw new Error('Guest file hash was invalid.');
  return { bytes, mode: Number.parseInt(modeText, 8), sha256 };
}

async function removeTartGuestTemporaryFile(vmName, path, timeoutSeconds, transport) {
  try {
    await runTartGuestCommand(vmName, ['/bin/rm', '-f', path], Math.min(timeoutSeconds, 15), transport);
  } catch {
    // The destination is random, bounded, and reported only through the original transfer error.
  }
}

async function waitForTartGuest(vmName, waitSeconds) {
  if (waitSeconds === 0) return { ready: false, detail: 'Guest-agent readiness wait was disabled.' };
  const deadline = Date.now() + waitSeconds * 1000;
  let lastDetail = 'Tart guest agent did not become ready.';
  while (Date.now() < deadline) {
    const remainingSeconds = Math.max(1, Math.ceil((deadline - Date.now()) / 1000));
    const baseline = await tartGuestBaseline(vmName, Math.min(5, remainingSeconds));
    if (baseline.ready) return baseline;
    if (baseline.restartRequired) return baseline;
    lastDetail = baseline.detail;
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 1_000));
  }
  return { ready: false, detail: lastDetail };
}

async function tartGuestBaseline(vmName, timeoutSeconds) {
  try {
    const transport = await resolveTartGuestTransport(vmName, timeoutSeconds);
    const productVersion = await runTartGuestProbe(vmName, ['/usr/bin/sw_vers', '-productVersion'], timeoutSeconds, false, transport);
    const buildVersion = await runTartGuestProbe(vmName, ['/usr/bin/sw_vers', '-buildVersion'], timeoutSeconds, false, transport);
    const architecture = await runTartGuestProbe(vmName, ['/usr/bin/uname', '-m'], timeoutSeconds, false, transport);
    const sip = await runTartGuestProbe(vmName, ['/usr/bin/csrutil', 'status'], timeoutSeconds, true, transport);
    return {
      ready: true,
      transport,
      productVersion: firstLine(productVersion.stdout),
      buildVersion: firstLine(buildVersion.stdout),
      architecture: firstLine(architecture.stdout),
      sip: firstLine(sip.stdout || sip.stderr) || 'unknown'
    };
  } catch (error) {
    return {
      ready: false,
      detail: publicError(error),
      ...(error instanceof TartGuestRestartRequiredError ? { restartRequired: true } : {})
    };
  }
}

async function runTartGuestProbe(vmName, argv, timeoutSeconds, allowFailure = false, transport = 'guest-agent') {
  try {
    return await runTartGuestCommand(vmName, argv, timeoutSeconds, transport);
  } catch (error) {
    if (allowFailure && error instanceof CommandError) return error.result;
    throw error;
  }
}

async function resolveTartGuestTransport(vmName, timeoutSeconds) {
  const restartRequired = tartGuestRestartRequired.get(vmName);
  if (restartRequired) throw new TartGuestRestartRequiredError(restartRequired);
  const transportPolicy = configuredTartTransportPolicy();
  const cached = tartGuestTransports.get(vmName);
  if (cached === 'ssh' && transportPolicy !== 'guest-agent-only') return cached;
  if (cached === 'ssh') tartGuestTransports.delete(vmName);
  try {
    await prepareTartGuestAgent(vmName, timeoutSeconds);
    tartGuestTransports.set(vmName, 'guest-agent');
    return 'guest-agent';
  } catch (guestAgentError) {
    if (isTartGuestDescriptorExhaustion(guestAgentError)) {
      tartGuestTransports.delete(vmName);
      const error = tartGuestDescriptorExhaustionError();
      tartGuestRestartRequired.set(vmName, error.message);
      throw error;
    }
    if (transportPolicy === 'guest-agent-only') {
      throw new Error(`Tart Guest Agent is required by the configured transport policy, but execution preparation failed (${publicError(guestAgentError)}). Prepare the VM clone source with Guest Agent RPC support; SSH and host command runners are disabled.`);
    }
    try {
      await runTartSshCommand(vmName, ['/usr/bin/true'], timeoutSeconds);
      tartGuestTransports.set(vmName, 'ssh');
      return 'ssh';
    } catch (sshError) {
      throw new Error(`Tart Guest Agent unavailable (${publicError(guestAgentError)}); configured SSH fallback unavailable (${publicError(sshError)}).`);
    }
  }
}

async function prepareTartGuestAgent(vmName, timeoutSeconds) {
  let helperPath = tartGuestExecHelpers.get(vmName);
  let descriptorCount = null;
  const operationsSinceProbe = tartGuestOperationsSinceDescriptorProbe.get(vmName)
    ?? TART_GUEST_DESCRIPTOR_PROBE_INTERVAL;
  if (helperPath && operationsSinceProbe < TART_GUEST_DESCRIPTOR_PROBE_INTERVAL) return;
  if (helperPath) {
    try {
      descriptorCount = await probeTartGuestDescriptors(vmName, timeoutSeconds, helperPath);
    } catch (error) {
      tartGuestExecHelpers.delete(vmName);
      helperPath = null;
      if (isTartGuestDescriptorExhaustion(error)) throw error;
    }
  }
  if (!helperPath) {
    descriptorCount = await probeTartGuestDescriptors(vmName, timeoutSeconds, null);
    if (descriptorCount >= TART_GUEST_DESCRIPTOR_RECOVERY_THRESHOLD) {
      await recycleTartGuestAgent(vmName, timeoutSeconds, null);
    }
    helperPath = await installTartGuestExecHelper(vmName, timeoutSeconds);
    descriptorCount = await probeTartGuestDescriptors(vmName, timeoutSeconds, helperPath);
  }
  if (descriptorCount >= TART_GUEST_DESCRIPTOR_RECOVERY_THRESHOLD) {
    await recycleTartGuestAgent(vmName, timeoutSeconds, helperPath);
    descriptorCount = await probeTartGuestDescriptors(vmName, timeoutSeconds, helperPath);
  }
  if (descriptorCount >= TART_GUEST_DESCRIPTOR_RECOVERY_THRESHOLD) {
    throw tartGuestDescriptorExhaustionError();
  }
  tartGuestOperationsSinceDescriptorProbe.set(vmName, 0);
}

async function installTartGuestExecHelper(vmName, timeoutSeconds) {
  const temporaryPath = `${TART_GUEST_EXEC_HELPER_PATH}-${randomUUID()}`;
  try {
    const hostHelperPath = await prepareTartHostExecHelper();
    await runCommandWithFileInput(
      tartCommand(),
      ['exec', '-i', vmName, '/bin/dd', `of=${temporaryPath}`, 'bs=65536'],
      hostHelperPath,
      { timeoutMs: Math.max(10, Math.min(timeoutSeconds, 60)) * 1000, maxBytes: 1024 * 1024 }
    );
    await runCommand(tartCommand(), ['exec', vmName, '/bin/chmod', '0700', temporaryPath], {
      timeoutMs: Math.max(3, Math.min(timeoutSeconds, 15)) * 1000
    });
    await runCommand(tartCommand(), ['exec', vmName, '/bin/mv', '-f', temporaryPath, TART_GUEST_EXEC_HELPER_PATH], {
      timeoutMs: Math.max(3, Math.min(timeoutSeconds, 15)) * 1000
    });
    tartGuestExecHelpers.set(vmName, TART_GUEST_EXEC_HELPER_PATH);
    return TART_GUEST_EXEC_HELPER_PATH;
  } catch (error) {
    tartGuestExecHelpers.delete(vmName);
    throw new Error(`Unable to install the native Tart guest exec helper: ${publicError(error)}`);
  }
}

async function prepareTartHostExecHelper() {
  if (tartHostExecHelperPromise) return tartHostExecHelperPromise;
  tartHostExecHelperPromise = (async () => {
    const testHelper = process.env.APPLE_SECURITY_TEST_PLATFORM === 'darwin'
      ? process.env.APPLE_SECURITY_TEST_TART_EXEC_HELPER
      : null;
    if (testHelper) return existingExecutable(testHelper, 'APPLE_SECURITY_TEST_TART_EXEC_HELPER');
    if (process.platform !== 'darwin' || process.arch !== 'arm64') {
      throw new Error('The native Tart guest exec helper requires an Apple Silicon macOS host.');
    }
    const sourceHash = createHash('sha256').update(TART_GUEST_EXEC_HELPER_SOURCE).digest('hex').slice(0, 16);
    const helperPath = join(PLUGIN_DATA, `tart-guest-exec-${sourceHash}`);
    if (existsSync(helperPath) && statSync(helperPath).isFile()) return helperPath;
    const temporaryPath = join(PLUGIN_DATA, `.tart-guest-exec-${randomUUID()}`);
    try {
      await runCommandWithBufferInput(
        '/usr/bin/clang',
        ['-Os', '-std=c11', '-arch', 'arm64', '-mmacosx-version-min=13.0', '-x', 'c', '-o', temporaryPath, '-'],
        Buffer.from(TART_GUEST_EXEC_HELPER_SOURCE, 'utf8'),
        { timeoutMs: 60_000 }
      );
      chmodSync(temporaryPath, 0o700);
      renameSync(temporaryPath, helperPath);
      return helperPath;
    } catch (error) {
      removeFileIfPresent(temporaryPath);
      throw new Error(`Unable to build the native Tart guest exec helper on the Beale host: ${publicError(error)}`);
    }
  })();
  try {
    return await tartHostExecHelperPromise;
  } catch (error) {
    tartHostExecHelperPromise = null;
    throw error;
  }
}

async function probeTartGuestDescriptors(vmName, timeoutSeconds, helperPath) {
  const argv = helperPath
    ? [helperPath, '--beale-probe']
    : ['/bin/ls', '-1', '/dev/fd'];
  const result = await runCommand(tartCommand(), ['exec', vmName, ...argv], {
    timeoutMs: Math.max(2, Math.min(timeoutSeconds, 10)) * 1000
  });
  if (helperPath) {
    const count = Number.parseInt(firstLine(result.stdout), 10);
    if (!Number.isSafeInteger(count) || count < 3) throw new Error('Tart guest exec helper returned an invalid descriptor count.');
    return count;
  }
  const descriptors = result.stdout.split(/\r?\n/u).filter((line) => /^\d+$/u.test(line));
  if (descriptors.length < 3) throw new Error('Tart Guest Agent descriptor probe returned an invalid result.');
  return descriptors.length;
}

async function recycleTartGuestAgent(vmName, timeoutSeconds, helperPath) {
  const prefix = helperPath ? [helperPath] : [];
  const services = await runCommand(
    tartCommand(),
    ['exec', vmName, ...prefix, '/bin/launchctl', 'print', 'system'],
    { timeoutMs: Math.max(3, Math.min(timeoutSeconds, 15)) * 1000 }
  );
  const label = services.stdout.split(/\r?\n/u).flatMap((line) => {
    const match = line.match(/^\s*[1-9]\d*\s+\S+\s+(org\.cirruslabs\.tart-guest(?:-agent|-rpc-[A-Za-z0-9._-]+))\s*$/u);
    return match ? [match[1]] : [];
  })[0];
  if (!label) {
    throw new Error('Tart Guest Agent descriptor pressure was detected, but its active launchd service could not be identified safely. Stop and restart the disposable VM before another guest operation.');
  }
  try {
    await runCommand(
      tartCommand(),
      ['exec', vmName, ...prefix, '/usr/bin/sudo', '-n', '/bin/launchctl', 'kill', 'SIGTERM', `system/${label}`],
      { timeoutMs: 2_000 }
    );
  } catch {
    // Killing the service also severs the RPC that requested the restart.
  }
  tartGuestTransports.delete(vmName);
  const deadline = Date.now() + Math.max(3, Math.min(timeoutSeconds, 15)) * 1000;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const count = await probeTartGuestDescriptors(vmName, 3, helperPath);
      if (count < TART_GUEST_DESCRIPTOR_RECOVERY_THRESHOLD) {
        tartGuestTransports.set(vmName, 'guest-agent');
        tartGuestOperationsSinceDescriptorProbe.set(vmName, 0);
        return;
      }
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 250));
  }
  throw new Error(`Tart Guest Agent did not recover after descriptor-pressure recycling${lastError ? ` (${publicError(lastError)})` : ''}. Stop and restart the disposable VM before another guest operation.`);
}

async function runTartGuestCommand(vmName, argv, timeoutSeconds, transport) {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const invocation = await tartGuestCommandInvocation(vmName, argv, timeoutSeconds, transport, false);
    try {
      const result = await runCommand(invocation.command, invocation.args, { timeoutMs: timeoutSeconds * 1000 });
      noteTartGuestOperation(vmName);
      return result;
    } catch (error) {
      noteTartGuestOperation(vmName);
      if (transport === 'guest-agent' && attempt === 0 && isMissingTartGuestExecHelper(error)) {
        const signal = requestSignalStorage.getStore();
        if (signal?.aborted) throw signal.reason ?? error;
        tartGuestExecHelpers.delete(vmName);
        tartGuestOperationsSinceDescriptorProbe.delete(vmName);
        await prepareTartGuestAgent(vmName, timeoutSeconds);
        continue;
      }
      if (transport === 'guest-agent' && isTartGuestDescriptorExhaustion(error)) {
        tartGuestTransports.delete(vmName);
        const restartRequired = tartGuestDescriptorExhaustionError();
        tartGuestRestartRequired.set(vmName, restartRequired.message);
        throw restartRequired;
      }
      throw error;
    }
  }
  throw new Error('Tart guest command did not complete.');
}

async function runTartSshCommand(vmName, argv, timeoutSeconds) {
  const invocation = await tartSshCommandInvocation(vmName, argv, timeoutSeconds);
  return runCommand(invocation.command, invocation.args, { timeoutMs: timeoutSeconds * 1000 });
}

async function tartGuestCommandInvocation(vmName, argv, timeoutSeconds, transport, attachInput) {
  if (transport === 'ssh') return tartSshCommandInvocation(vmName, argv, timeoutSeconds);
  const helperPath = tartGuestExecHelpers.get(vmName);
  if (!helperPath) throw new Error('The native Tart guest exec helper is not prepared.');
  return {
    command: tartCommand(),
    args: [
      'exec',
      ...(attachInput ? ['-i'] : []),
      vmName,
      helperPath,
      ...argv
    ]
  };
}

async function tartSshCommandInvocation(vmName, argv, timeoutSeconds) {
  const { ssh, address, connectTimeout } = await tartSshEndpoint(vmName, timeoutSeconds);
  const remoteCommand = argv.map(posixShellQuote).join(' ');
  const sshArgs = [
    '-o', 'BatchMode=yes',
    '-o', 'IdentitiesOnly=yes',
    '-o', 'StrictHostKeyChecking=yes',
    '-o', `UserKnownHostsFile=${ssh.knownHosts}`,
    '-o', `ConnectTimeout=${connectTimeout}`,
    '-o', 'ConnectionAttempts=1',
    '-i', ssh.identity,
    `${ssh.user}@${address}`,
    '--', remoteCommand
  ];
  return hostCommandInvocation(ssh.command, sshArgs);
}

async function tartScpUploadInvocation(vmName, localPath, guestPath, timeoutSeconds, commandRunner) {
  const { ssh, address, connectTimeout } = await tartSshEndpoint(vmName, timeoutSeconds);
  const remoteHost = address.includes(':') ? `[${address}]` : address;
  const scpArgs = [
    '-o', 'BatchMode=yes',
    '-o', 'IdentitiesOnly=yes',
    '-o', 'StrictHostKeyChecking=yes',
    '-o', `UserKnownHostsFile=${ssh.knownHosts}`,
    '-o', `ConnectTimeout=${connectTimeout}`,
    '-o', 'ConnectionAttempts=1',
    '-i', ssh.identity,
    '--', localPath, `${ssh.user}@${remoteHost}:${guestPath}`
  ];
  const command = process.env.APPLE_SECURITY_SCP_COMMAND || '/usr/bin/scp';
  return hostCommandInvocation(command, scpArgs, commandRunner);
}

async function tartSshEndpoint(vmName, timeoutSeconds) {
  const ssh = tartSshConfig();
  const addressResult = await runCommand(tartCommand(), ['ip', vmName, '--wait', String(Math.min(10, timeoutSeconds))], {
    timeoutMs: (Math.min(10, timeoutSeconds) + 5) * 1000
  });
  const address = addressResult.stdout.trim();
  if (!/^[0-9A-Fa-f:.]+$/u.test(address) || address.length > 128) throw new Error('Tart did not return a valid bounded VM address for SSH fallback.');
  const connectTimeout = String(Math.max(1, Math.min(8, timeoutSeconds)));
  return { ssh, address, connectTimeout };
}

function tartSshConfig() {
  const identity = resolve(process.env.APPLE_SECURITY_SSH_IDENTITY || join(homedir(), '.ssh', 'id_ed25519'));
  const knownHosts = resolve(process.env.APPLE_SECURITY_SSH_KNOWN_HOSTS || join(homedir(), '.ssh', 'tart_known_hosts'));
  if (!existsSync(identity) || !statSync(identity).isFile()) throw new Error('SSH fallback identity is not configured.');
  if (!existsSync(knownHosts) || !statSync(knownHosts).isFile()) throw new Error('SSH fallback known-hosts file is not configured.');
  const user = process.env.APPLE_SECURITY_SSH_USER || 'admin';
  if (!/^[A-Za-z_][A-Za-z0-9._-]{0,63}$/u.test(user)) throw new Error('SSH fallback user is invalid.');
  return {
    command: process.env.APPLE_SECURITY_SSH_COMMAND || '/usr/bin/ssh',
    identity,
    knownHosts,
    user
  };
}

function posixShellQuote(value) {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function isTartGuestDescriptorExhaustion(error) {
  const detail = error instanceof CommandError
    ? `${error.result.stderr}\n${error.result.stdout}`
    : error instanceof Error ? error.message : String(error);
  return /(?:too many open files|unable to create pipe|cannot duplicate fd|descriptor exhaustion|descriptor-pressure recycling|did not recover after descriptor)/iu.test(
    detail
  );
}

function isMissingTartGuestExecHelper(error) {
  const detail = error instanceof CommandError
    ? `${error.result.stderr}\n${error.result.stdout}`
    : error instanceof Error ? error.message : String(error);
  return detail.includes(TART_GUEST_EXEC_HELPER_PATH)
    && /(?:no such file or directory|fork\/exec.*unknown \(2\)|\benoent\b)/iu.test(detail);
}

function tartGuestDescriptorExhaustionError() {
  return new TartGuestRestartRequiredError('Tart Guest Agent descriptor exhaustion was detected and safe recycling could not be completed. The requested command was not replayed. Guest execution is now latched off for this VM; stop and restart the disposable VM once before another guest operation.');
}

class TartGuestRestartRequiredError extends Error {}

function noteTartGuestOperation(vmName) {
  tartGuestOperationsSinceDescriptorProbe.set(
    vmName,
    (tartGuestOperationsSinceDescriptorProbe.get(vmName) ?? 0) + 1
  );
}

function hostCommandInvocation(command, args, configuredRunner) {
  const runner = configuredRunner ?? configuredHostCommandRunner();
  return runner
    ? { command: runner, args: ['run', '--', command, ...args] }
    : { command, args };
}

function configuredHostCommandRunner() {
  const override = process.env.APPLE_SECURITY_COMMAND_RUNNER;
  if (override) return existingExecutable(override, 'APPLE_SECURITY_COMMAND_RUNNER');
  const configured = readAppleSecurityHostConfig();
  return typeof configured.commandRunner === 'string'
    ? existingExecutable(configured.commandRunner, 'commandRunner')
    : null;
}

function configuredTartTransportPolicy() {
  const override = process.env.APPLE_SECURITY_TART_TRANSPORT_POLICY;
  const configured = override || readAppleSecurityHostConfig().tartTransportPolicy || 'guest-agent-or-ssh';
  if (configured !== 'guest-agent-only' && configured !== 'guest-agent-or-ssh') {
    throw new Error('Tart transport policy must be guest-agent-only or guest-agent-or-ssh.');
  }
  return configured;
}

function readAppleSecurityHostConfig() {
  const configPath = join(PLUGIN_DATA, 'host-config.json');
  if (!existsSync(configPath)) return {};
  const parsed = parseJson(readFileSync(configPath, 'utf8'), 'Apple security devices host config');
  if (!isRecord(parsed)) throw new Error('Apple security devices host config must be an object.');
  if (parsed.commandRunner !== undefined && typeof parsed.commandRunner !== 'string') {
    throw new Error('Apple security devices host config commandRunner must be a string.');
  }
  if (parsed.tartTransportPolicy !== undefined && typeof parsed.tartTransportPolicy !== 'string') {
    throw new Error('Apple security devices host config tartTransportPolicy must be a string.');
  }
  return parsed;
}

function existingExecutable(value, field) {
  const path = resolve(requiredString(value, field, 4096));
  if (!existsSync(path) || !statSync(path).isFile()) throw new Error(`${field} must identify an existing executable file.`);
  return realpathSync(path);
}

function requireKnownTartVm(vms, vmName) {
  const vm = vms.find((candidate) => candidate.name === vmName);
  if (!vm) throw new Error(`Unknown Tart VM: ${vmName}. Call list_tart_vms and select an existing VM by exact name.`);
  return vm;
}

async function listPhysicalIphones(args) {
  assertDarwinHost('physical iPhone');
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 5, 60, 15);
  const parsed = await runDevicectl(['list', 'devices'], timeoutSeconds);
  const devices = findDeviceObjects(parsed).filter(isPhysicalIphone);
  deviceRefs.clear();
  return {
    devices: devices.slice(0, 32).map((device) => {
      const identifier = deviceIdentifier(device);
      const deviceRef = opaqueDeviceRef(identifier);
      deviceRefs.set(deviceRef, identifier);
      return sanitizeDevice(device, deviceRef);
    }),
    simulatorDevicesExcluded: true,
    note: 'Device references are opaque and expire when this MCP server restarts or devices are re-listed.'
  };
}

async function describePhysicalIphone(args) {
  assertDarwinHost('physical iPhone');
  const identifier = resolveDeviceRef(args.deviceRef);
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 5, 60, 15);
  const parsed = await runDevicectl(['device', 'info', 'details', '--device', identifier], timeoutSeconds);
  const device = findDeviceObjects(parsed).find((candidate) => deviceIdentifier(candidate) === identifier)
    ?? findDeviceObjects(parsed).find(isPhysicalIphone);
  if (!device || !isPhysicalIphone(device)) throw new Error('CoreDevice did not return a physical iPhone for that reference.');
  return sanitizeDevice(device, args.deviceRef, true);
}

async function installPhysicalIphoneApp(args) {
  assertDarwinHost('physical iPhone');
  const identifier = resolveDeviceRef(args.deviceRef);
  const appPath = canonicalDirectory(args.appPath, 'appPath');
  if (!basename(appPath).toLocaleLowerCase().endsWith('.app')) throw new Error('appPath must identify an existing .app bundle directory.');
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 5, 300, 120);
  await runDevicectl(['device', 'install', 'app', '--device', identifier], timeoutSeconds, [appPath]);
  return { installed: true, deviceRef: args.deviceRef, appBundle: basename(appPath) };
}

async function launchPhysicalIphoneApp(args) {
  assertDarwinHost('physical iPhone');
  const identifier = resolveDeviceRef(args.deviceRef);
  const bundleIdentifier = requiredString(args.bundleIdentifier, 'bundleIdentifier', 255);
  if (!/^[A-Za-z0-9][A-Za-z0-9.-]*$/u.test(bundleIdentifier)) throw new Error('bundleIdentifier contains unsupported characters.');
  const launchArgs = ['device', 'process', 'launch', '--device', identifier];
  if (args.startStopped === true) launchArgs.push('--start-stopped');
  if (args.terminateExisting === true) launchArgs.push('--terminate-existing');
  if (args.environment !== undefined) launchArgs.push('--environment-variables', JSON.stringify(safeEnvironment(args.environment)));
  const timeoutSeconds = boundedInteger(args.timeoutSeconds, 5, 300, 60);
  await runDevicectl(launchArgs, timeoutSeconds, [bundleIdentifier, ...safeOptionalArgv(args.arguments)]);
  return {
    launched: true,
    deviceRef: args.deviceRef,
    bundleIdentifier,
    startStopped: args.startStopped === true,
    terminateExisting: args.terminateExisting === true
  };
}

async function darwinVmEnvironmentStatus() {
  const configured = Boolean(process.env.BEALE_DARWIN_VM_CHECKOUT);
  if (process.platform === 'win32') return { available: false, configured, detail: 'The Darwin VM launcher requires a macOS or Linux app-server host.' };
  if (!configured) return { available: false, configured, detail: 'Use the session-start setup dialog or supply a prepared checkoutRoot to inspect_darwin_vm.' };
  try {
    const inspection = await inspectDarwinVm({});
    return { available: inspection.ready, configured, errors: inspection.errors, detail: 'Artifact checks do not prove boot success or device/build compatibility.' };
  } catch {
    return { available: false, configured, detail: 'The saved checkout is unavailable. Reconfigure it or supply a prepared checkoutRoot to inspect_darwin_vm.' };
  }
}

async function inspectDarwinVm(args) {
  const checkoutRoot = canonicalDirectory(args.checkoutRoot ?? process.env.BEALE_DARWIN_VM_CHECKOUT, 'checkoutRoot');
  const inspection = await inspectDarwinCheckout(checkoutRoot, args.hashArtifacts === true);
  return {
    ready: inspection.errors.length === 0,
    artifacts: inspection.artifacts,
    errors: inspection.errors,
    warnings: inspection.warnings,
    posture: 'Barebones, modified Darwin root-shell lab; not a full iPhone or Mac emulator.'
  };
}

function listDarwinVmRuns() {
  return {
    runs: readRunRecords().map(publicRunRecord),
    note: 'Only runs started by this plugin are listed.'
  };
}

function readDarwinVmLog(args) {
  const record = readRunRecord(args.runId);
  const maxBytes = boundedInteger(args.maxBytes, 1024, MAX_LOG_BYTES, 32768);
  return {
    run: publicRunRecord(record),
    serialLog: readTail(record.serialLogPath, maxBytes),
    truncatedToBytes: maxBytes
  };
}

async function startDarwinVm(args) {
  const checkoutRoot = canonicalDirectory(args.checkoutRoot ?? process.env.BEALE_DARWIN_VM_CHECKOUT, 'checkoutRoot');
  const inspection = await inspectDarwinCheckout(checkoutRoot, false);
  if (inspection.errors.length) throw new Error(`darwin-vm checkout is not ready: ${inspection.errors.join(' ')}`);
  const memoryMiB = boundedInteger(args.memoryMiB, 2048, 32768, 8192);
  const bootArguments = args.bootArguments === undefined
    ? 'rd=md0 serial=3 -v -noprogress wdt=-1 wlan-olyhal-abort'
    : requiredSingleLine(args.bootArguments, 'bootArguments', 2048);
  const runId = `darwin_${randomUUID()}`;
  const runRoot = join(RUNS_ROOT, runId);
  mkdirSync(runRoot, { recursive: true, mode: 0o700 });
  const serialSocketPath = join(tmpdir(), `beale-darwin-${runId.slice(-12)}.sock`);
  const serialLogPath = join(runRoot, 'serial.log');
  const qemuLogPath = join(runRoot, 'qemu.log');
  const firmware = join(checkoutRoot, 'firmware');
  const qemu = join(checkoutRoot, 'qemu-sptm', 'build', 'qemu-system-aarch64');
  const qemuArgs = [
    '-M', 'darwin',
    '-bootkc', join(firmware, 'bootkc'),
    '-dtree', join(firmware, 'dtree'),
    '-tc', join(firmware, 'ramdisk.tc'),
    '-ramdisk', join(firmware, 'ramdisk.dmg'),
    '-args', bootArguments,
    '-nic', 'none',
    '-display', 'none',
    '-monitor', 'none',
    '-chardev', `socket,id=serial0,path=${serialSocketPath},server=on,wait=off,logfile=${serialLogPath},logappend=on`,
    '-serial', 'chardev:serial0',
    '-m', `${memoryMiB}M`
  ];
  if (inspection.hasSptm) qemuArgs.push('-sptm', join(firmware, 'sptm'), '-txm', join(firmware, 'txm'));
  const child = await spawnDetached(qemu, qemuArgs, { cwd: checkoutRoot, logPath: qemuLogPath });
  const record = {
    version: 1,
    runId,
    pid: child.pid,
    checkoutRoot,
    serialSocketPath,
    serialLogPath,
    qemuLogPath,
    startedAt: new Date().toISOString(),
    memoryMiB,
    state: 'running'
  };
  writeRunRecord(record);
  activeDarwinRuns.set(runId, record);
  child.once('exit', (code, signal) => {
    try {
      activeDarwinRuns.delete(runId);
      writeRunRecord({
        ...record,
        state: 'stopped',
        exitedAt: new Date().toISOString(),
        exitCode: Number.isInteger(code) ? code : null,
        exitSignal: signal ?? null
      });
    } catch {
      // The process is already detached; lifecycle bookkeeping is best effort.
    }
  });
  return {
    started: true,
    run: publicRunRecord(record),
    posture: {
      networkDevice: false,
      hostShares: false,
      graphics: false,
      qemuMonitor: false,
      serialConsole: true
    }
  };
}

async function stopDarwinVm(args) {
  const record = activeDarwinRun(args.runId);
  const running = isPidRunning(record.pid);
  if (running) process.kill(record.pid, 'SIGTERM');
  const updated = {
    ...record,
    state: 'stopped',
    stoppedAt: new Date().toISOString(),
    stopRequested: running
  };
  writeRunRecord(updated);
  activeDarwinRuns.delete(record.runId);
  return { stopped: true, run: publicRunRecord(updated) };
}

async function runDarwinVmConsoleCommand(args) {
  const record = activeDarwinRun(args.runId);
  if (!isPidRunning(record.pid)) throw new Error('The selected darwin-vm run is not running.');
  const command = requiredSingleLine(args.command, 'command', 1024);
  const readMilliseconds = boundedInteger(args.readMilliseconds, 100, 5000, 1000);
  const output = await exchangeUnixSocket(record.serialSocketPath, `${command}\n`, readMilliseconds);
  return { runId: record.runId, commandSent: true, output };
}

async function inspectDarwinCheckout(checkoutRoot, hashArtifacts) {
  const definitions = [
    ['qemu', 'qemu-sptm/build/qemu-system-aarch64', true],
    ['bootkc', 'firmware/bootkc', true],
    ['deviceTree', 'firmware/dtree', true],
    ['trustCache', 'firmware/ramdisk.tc', true],
    ['ramdisk', 'firmware/ramdisk.dmg', true],
    ['sptm', 'firmware/sptm', false],
    ['txm', 'firmware/txm', false]
  ];
  const artifacts = [];
  const errors = [];
  for (const [name, relativePath, required] of definitions) {
    const candidate = containedExistingFile(checkoutRoot, relativePath);
    if (!candidate) {
      artifacts.push({ name, present: false, required });
      if (required) errors.push(`Missing required artifact ${relativePath}.`);
      continue;
    }
    const stats = statSync(candidate);
    if (required && stats.size === 0) errors.push(`Empty required artifact ${relativePath}.`);
    artifacts.push({
      name,
      present: true,
      required,
      sizeBytes: stats.size,
      ...(hashArtifacts ? { sha256: await sha256File(candidate) } : {})
    });
  }
  const hasSptm = artifacts.some((item) => item.name === 'sptm' && item.present);
  const hasTxm = artifacts.some((item) => item.name === 'txm' && item.present);
  const warnings = [];
  if (hasSptm !== hasTxm) warnings.push('SPTM and TXM must be supplied together; the incomplete pair will not be used.');
  warnings.push('Artifact presence does not prove device, OS build, kernel collection, device tree, SPTM, or TXM compatibility. Verify the exact profile before interpreting results.');
  return { artifacts, errors, warnings, hasSptm: hasSptm && hasTxm };
}

async function runDevicectl(commandArgs, timeoutSeconds, trailingArgs = []) {
  const developerDir = findDeveloperDir();
  if (!developerDir) throw new Error('A full Xcode developer directory with CoreDevice was not found.');
  const result = await runCommand(xcrunCommand(), [
    'devicectl', ...commandArgs,
    '--quiet',
    '--timeout', String(timeoutSeconds),
    '--json-output', '-',
    ...trailingArgs
  ], {
    env: { DEVELOPER_DIR: developerDir },
    timeoutMs: (timeoutSeconds + 5) * 1000
  });
  const parsed = parseJson(result.stdout, 'CoreDevice JSON output');
  const outcome = isRecord(parsed?.info) && typeof parsed.info.outcome === 'string' ? parsed.info.outcome : null;
  if (isRecord(parsed?.error) || (outcome && outcome !== 'success')) {
    throw new Error(`CoreDevice operation did not succeed${outcome ? ` (${outcome})` : ''}.`);
  }
  return parsed;
}

function findDeveloperDir() {
  const override = process.env.APPLE_SECURITY_DEVELOPER_DIR;
  if (override && isDirectory(override)) return realpathSync(override);
  if (process.platform !== 'darwin') return null;
  try {
    const selected = runCommandSync('/usr/bin/xcode-select', ['-p']).trim();
    if (selected.includes('.app/Contents/Developer') && isDirectory(selected)) return realpathSync(selected);
  } catch {
    // Fall through to installed Xcode discovery.
  }
  try {
    const candidates = readdirSync('/Applications')
      .filter((name) => /^Xcode[^/]*\.app$/u.test(name))
      .sort((left, right) => right.localeCompare(left));
    for (const name of candidates) {
      const candidate = join('/Applications', name, 'Contents', 'Developer');
      if (isDirectory(candidate)) return realpathSync(candidate);
    }
  } catch {
    // No readable Applications directory.
  }
  return null;
}

function findDeviceObjects(value) {
  const objects = [];
  const visit = (current, depth) => {
    if (depth > 8) return;
    if (Array.isArray(current)) {
      for (const item of current) visit(item, depth + 1);
      return;
    }
    if (!isRecord(current)) return;
    if (deviceIdentifier(current) && looksLikeDevice(current)) objects.push(current);
    for (const child of Object.values(current)) visit(child, depth + 1);
  };
  visit(value, 0);
  return dedupeBy(objects, deviceIdentifier);
}

function looksLikeDevice(device) {
  const text = JSON.stringify({
    hardwareProperties: device.hardwareProperties,
    deviceProperties: device.deviceProperties,
    properties: device.properties,
    platform: device.platform,
    productType: device.productType
  });
  return /(?:iphone|ios|deviceProperties|hardwareProperties)/iu.test(text);
}

function isPhysicalIphone(device) {
  const searchable = flattenScalarEntries(device)
    .filter(([key]) => /(?:platform|product|device|family|kind|type|class|simulator)/iu.test(key))
    .map(([key, value]) => `${key}=${value}`)
    .join(' ');
  return /(?:\biphone\b|productType=iPhone\d)/iu.test(searchable)
    && /(?:\bios\b|platform=iOS|iphone)/iu.test(searchable)
    && !SIMULATOR_PATTERN.test(searchable)
    && !/(?:isSimulator=true|virtual=true)/iu.test(searchable);
}

function sanitizeDevice(device, deviceRef, detailed = false) {
  const fields = scalarFieldMap(device);
  const result = {
    deviceRef,
    productType: pickField(fields, ['hardwareProperties.productType', 'productType']),
    productName: pickField(fields, ['hardwareProperties.marketingName', 'hardwareProperties.productName']),
    platform: pickField(fields, ['hardwareProperties.platform', 'platform']) ?? 'iOS',
    osVersion: pickField(fields, ['deviceProperties.osVersionNumber', 'operatingSystemVersion', 'osVersion']),
    osBuild: pickField(fields, ['deviceProperties.osBuildUpdate', 'osBuild', 'buildVersion']),
    connection: pickField(fields, ['connectionProperties.transportType', 'transportType', 'connectionType']),
    state: pickField(fields, ['deviceProperties.bootState', 'state', 'visibilityClass'])
  };
  if (detailed) {
    result.developerMode = pickField(fields, ['deviceProperties.developerModeStatus', 'developerModeStatus']);
    result.tunnelState = pickField(fields, ['connectionProperties.tunnelState', 'tunnelState']);
    result.paired = pickField(fields, ['connectionProperties.pairingState', 'pairingState', 'paired']);
  }
  return result;
}

function deviceIdentifier(device) {
  const fields = scalarFieldMap(device);
  const identifier = pickField(fields, [
    'identifier',
    'deviceProperties.identifier',
    'hardwareProperties.udid',
    'udid',
    'serialNumber'
  ]);
  return typeof identifier === 'string' && identifier ? identifier : null;
}

function opaqueDeviceRef(identifier) {
  return `device_${createHash('sha256').update(`${deviceRefSalt}:${identifier}`).digest('hex').slice(0, 20)}`;
}

function resolveDeviceRef(value) {
  const deviceRef = requiredString(value, 'deviceRef', 80);
  const identifier = deviceRefs.get(deviceRef);
  if (!identifier) throw new Error('Unknown or expired physical-iPhone deviceRef. Call list_physical_iphones again.');
  return identifier;
}

function readRunRecords() {
  if (!existsSync(RUNS_ROOT)) return [];
  return readdirSync(RUNS_ROOT, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && entry.name.startsWith('darwin_'))
    .flatMap((entry) => {
      try {
        return [readRunRecord(entry.name)];
      } catch {
        return [];
      }
    })
    .sort((left, right) => right.startedAt.localeCompare(left.startedAt));
}

function readRunRecord(value) {
  const runId = requiredString(value, 'runId', 80);
  if (!/^darwin_[0-9a-f-]{36}$/u.test(runId)) throw new Error('runId is invalid.');
  const runRoot = resolve(RUNS_ROOT, runId);
  assertContained(RUNS_ROOT, runRoot, 'runId');
  const recordPath = join(runRoot, 'run.json');
  if (!existsSync(recordPath)) throw new Error('Unknown darwin-vm runId.');
  const record = JSON.parse(readFileSync(recordPath, 'utf8'));
  if (!isRecord(record) || record.runId !== runId || !Number.isInteger(record.pid)) throw new Error('darwin-vm run record is invalid.');
  assertContained(runRoot, resolve(record.serialLogPath), 'serial log');
  assertContained(runRoot, resolve(record.qemuLogPath), 'QEMU log');
  const expectedSocketPath = join(tmpdir(), `beale-darwin-${runId.slice(-12)}.sock`);
  if (record.serialSocketPath !== expectedSocketPath) throw new Error('darwin-vm run record has an invalid serial socket.');
  return record;
}

function activeDarwinRun(value) {
  const runId = requiredString(value, 'runId', 80);
  const record = activeDarwinRuns.get(runId);
  if (!record) throw new Error('The darwin-vm run is not active in this MCP process. Re-list runs and stop an orphaned QEMU process through an operator-controlled host workflow.');
  return record;
}

function writeRunRecord(record) {
  const runRoot = resolve(RUNS_ROOT, record.runId);
  assertContained(RUNS_ROOT, runRoot, 'runId');
  mkdirSync(runRoot, { recursive: true, mode: 0o700 });
  writeFileSync(join(runRoot, 'run.json'), `${JSON.stringify(record, null, 2)}\n`, { encoding: 'utf8', mode: 0o600 });
}

function publicRunRecord(record) {
  const running = record.state === 'running' && isPidRunning(record.pid);
  return {
    runId: record.runId,
    state: running ? 'running' : 'stopped',
    startedAt: record.startedAt,
    stoppedAt: record.stoppedAt ?? record.exitedAt ?? null,
    memoryMiB: record.memoryMiB
  };
}

function exchangeUnixSocket(socketPath, input, readMilliseconds) {
  return new Promise((resolvePromise, rejectPromise) => {
    let output = '';
    let settled = false;
    const socket = net.createConnection({ path: socketPath });
    const finish = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      if (error) rejectPromise(error);
      else resolvePromise(output);
    };
    const timer = setTimeout(() => finish(), readMilliseconds);
    socket.setEncoding('utf8');
    socket.on('connect', () => socket.write(input));
    socket.on('data', (chunk) => {
      output += chunk;
      if (Buffer.byteLength(output, 'utf8') >= MAX_CONSOLE_OUTPUT_BYTES) finish();
    });
    socket.on('error', (error) => finish(error));
    socket.on('end', () => finish());
  });
}

function readTail(path, maxBytes) {
  if (!existsSync(path)) return '';
  const file = readFileSync(path);
  return file.subarray(Math.max(0, file.length - maxBytes)).toString('utf8');
}

function runCommand(command, args, options = {}) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: { ...process.env, ...(options.env ?? {}) },
      shell: false,
      stdio: ['ignore', 'pipe', 'pipe']
    });
    let stdout = Buffer.alloc(0);
    let stderr = Buffer.alloc(0);
    let exceeded = false;
    const append = (existing, chunk) => {
      const combined = Buffer.concat([existing, chunk]);
      if (combined.length <= MAX_COMMAND_OUTPUT_BYTES) return combined;
      exceeded = true;
      return combined.subarray(combined.length - MAX_COMMAND_OUTPUT_BYTES);
    };
    child.stdout.on('data', (chunk) => { stdout = append(stdout, Buffer.from(chunk)); });
    child.stderr.on('data', (chunk) => { stderr = append(stderr, Buffer.from(chunk)); });
    const removeAbortListener = stopChildWhenRequestAborts(child, options);
    const timer = setTimeout(() => child.kill('SIGKILL'), options.timeoutMs ?? 30_000);
    child.once('error', (error) => {
      clearTimeout(timer);
      removeAbortListener();
      rejectPromise(error);
    });
    child.once('close', (code, signal) => {
      clearTimeout(timer);
      removeAbortListener();
      const result = {
        code: Number.isInteger(code) ? code : null,
        signal: signal ?? null,
        stdout: stdout.toString('utf8'),
        stderr: stderr.toString('utf8'),
        outputTruncated: exceeded
      };
      if (code === 0) resolvePromise(result);
      else rejectPromise(new CommandError(command, result));
    });
  });
}

function runCommandWithBufferInput(command, args, input, options = {}) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: { ...process.env, ...(options.env ?? {}) },
      shell: false,
      stdio: ['pipe', 'pipe', 'pipe']
    });
    let stdout = Buffer.alloc(0);
    let stderr = Buffer.alloc(0);
    let exceeded = false;
    let settled = false;
    let inputError = null;
    const append = (existing, chunk) => {
      const combined = Buffer.concat([existing, chunk]);
      if (combined.length <= MAX_COMMAND_OUTPUT_BYTES) return combined;
      exceeded = true;
      return combined.subarray(combined.length - MAX_COMMAND_OUTPUT_BYTES);
    };
    child.stdout.on('data', (chunk) => { stdout = append(stdout, Buffer.from(chunk)); });
    child.stderr.on('data', (chunk) => { stderr = append(stderr, Buffer.from(chunk)); });
    child.stdin.once('error', (error) => {
      if (error?.code !== 'EPIPE') inputError = error;
    });
    child.stdin.end(input);
    const removeAbortListener = stopChildWhenRequestAborts(child, options);
    const timer = setTimeout(() => child.kill('SIGKILL'), options.timeoutMs ?? 30_000);
    child.once('error', (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      removeAbortListener();
      rejectPromise(error);
    });
    child.once('close', (code, signal) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      removeAbortListener();
      if (inputError) {
        rejectPromise(inputError);
        return;
      }
      const result = {
        code: Number.isInteger(code) ? code : null,
        signal: signal ?? null,
        stdout: stdout.toString('utf8'),
        stderr: stderr.toString('utf8'),
        outputTruncated: exceeded
      };
      if (code === 0) resolvePromise(result);
      else rejectPromise(new CommandError(command, result));
    });
  });
}

function runCommandWithFileInput(command, args, inputPath, options = {}) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: { ...process.env, ...(options.env ?? {}) },
      shell: false,
      stdio: ['pipe', 'pipe', 'pipe']
    });
    const input = createReadStream(inputPath);
    let stdout = Buffer.alloc(0);
    let stderr = Buffer.alloc(0);
    let exceeded = false;
    let streamError = null;
    let settled = false;
    let inputBytes = 0;
    const maximum = options.maxBytes ?? DEFAULT_TART_COPY_BYTES;
    const append = (existing, chunk) => {
      const combined = Buffer.concat([existing, chunk]);
      if (combined.length <= MAX_COMMAND_OUTPUT_BYTES) return combined;
      exceeded = true;
      return combined.subarray(combined.length - MAX_COMMAND_OUTPUT_BYTES);
    };
    const failStream = (error) => {
      if (error?.code === 'EPIPE') return;
      streamError = error;
      child.kill('SIGKILL');
    };
    input.once('error', failStream);
    child.stdin.once('error', failStream);
    input.on('data', (chunk) => {
      inputBytes += chunk.length;
      if (inputBytes <= maximum || streamError) return;
      streamError = new Error(`Transfer exceeded the ${maximum}-byte limit.`);
      input.unpipe(child.stdin);
      input.destroy();
      child.stdin.destroy();
      child.kill('SIGKILL');
    });
    input.pipe(child.stdin);
    child.stdout.on('data', (chunk) => { stdout = append(stdout, Buffer.from(chunk)); });
    child.stderr.on('data', (chunk) => { stderr = append(stderr, Buffer.from(chunk)); });
    const removeAbortListener = stopChildWhenRequestAborts(child, options);
    const timer = setTimeout(() => child.kill('SIGKILL'), options.timeoutMs ?? 30_000);
    child.once('error', (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      removeAbortListener();
      input.destroy();
      rejectPromise(error);
    });
    child.once('close', (code, signal) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      removeAbortListener();
      input.destroy();
      if (streamError) {
        rejectPromise(streamError);
        return;
      }
      const result = {
        code: Number.isInteger(code) ? code : null,
        signal: signal ?? null,
        stdout: stdout.toString('utf8'),
        stderr: stderr.toString('utf8'),
        outputTruncated: exceeded
      };
      if (code === 0) resolvePromise(result);
      else rejectPromise(new CommandError(command, result));
    });
  });
}

async function runCommandToFile(command, args, outputPath, options = {}) {
  const child = spawn(command, args, {
    cwd: options.cwd,
    env: { ...process.env, ...(options.env ?? {}) },
    shell: false,
    stdio: ['ignore', 'pipe', 'pipe']
  });
  let stderr = Buffer.alloc(0);
  let stderrTruncated = false;
  const appendStderr = (chunk) => {
    const combined = Buffer.concat([stderr, Buffer.from(chunk)]);
    if (combined.length <= MAX_COMMAND_OUTPUT_BYTES) stderr = combined;
    else {
      stderrTruncated = true;
      stderr = combined.subarray(combined.length - MAX_COMMAND_OUTPUT_BYTES);
    }
  };
  child.stderr.on('data', appendStderr);
  let bytes = 0;
  const maximum = options.maxBytes ?? DEFAULT_TART_COPY_BYTES;
  const limiter = new Transform({
    transform(chunk, _encoding, callback) {
      bytes += chunk.length;
      if (bytes > maximum) callback(new Error(`Transfer exceeded the ${maximum}-byte limit.`));
      else callback(null, chunk);
    }
  });
  const outputPromise = pipeline(
    child.stdout,
    limiter,
    createWriteStream(outputPath, { flags: 'wx', mode: 0o600 })
  );
  const removeAbortListener = stopChildWhenRequestAborts(child, options);
  const closePromise = new Promise((resolvePromise, rejectPromise) => {
    const timer = setTimeout(() => child.kill('SIGKILL'), options.timeoutMs ?? 30_000);
    child.once('error', (error) => {
      clearTimeout(timer);
      removeAbortListener();
      rejectPromise(error);
    });
    child.once('close', (code, signal) => {
      clearTimeout(timer);
      removeAbortListener();
      resolvePromise({ code: Number.isInteger(code) ? code : null, signal: signal ?? null });
    });
  });
  let result;
  try {
    [result] = await Promise.all([closePromise, outputPromise]);
  } catch (error) {
    child.kill('SIGKILL');
    removeFileIfPresent(outputPath);
    throw error;
  }
  const commandResultValue = {
    ...result,
    stdout: '',
    stderr: stderr.toString('utf8'),
    outputTruncated: stderrTruncated
  };
  if (result.code !== 0) {
    removeFileIfPresent(outputPath);
    throw new CommandError(command, commandResultValue);
  }
  return { ...commandResultValue, bytes };
}

function stopChildWhenRequestAborts(child, options) {
  const signal = options.signal ?? requestSignalStorage.getStore();
  if (!signal) return () => {};
  const stop = () => child.kill('SIGKILL');
  if (signal.aborted) stop();
  else signal.addEventListener('abort', stop, { once: true });
  return () => signal.removeEventListener('abort', stop);
}

function runCommandSync(command, args) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    shell: false,
    stdio: ['ignore', 'pipe', 'pipe'],
    timeout: 5000
  });
  if (result.error || result.status !== 0) throw result.error ?? new Error('Command failed.');
  return result.stdout;
}

function spawnDetached(command, args, { cwd, logPath, startupGraceMs = 0 }) {
  return new Promise((resolvePromise, rejectPromise) => {
    const logFd = openSync(logPath, 'a', 0o600);
    const child = spawn(command, args, {
      cwd,
      env: process.env,
      shell: false,
      detached: true,
      stdio: ['ignore', logFd, logFd]
    });
    closeSync(logFd);
    let settled = false;
    child.once('error', (error) => {
      if (settled) return;
      settled = true;
      rejectPromise(error);
    });
    child.once('exit', (code, signal) => {
      if (settled) return;
      settled = true;
      const detail = firstLines(readTail(logPath, 4096), 4);
      rejectPromise(new Error(`${basename(command)} exited during startup with ${code === null ? `signal ${signal ?? 'unknown'}` : `code ${code}`}${detail ? `: ${detail}` : ''}`));
    });
    child.once('spawn', () => {
      if (settled) return;
      const finish = () => {
        if (settled) return;
        settled = true;
        child.unref();
        resolvePromise(child);
      };
      if (startupGraceMs > 0) setTimeout(finish, startupGraceMs);
      else finish();
    });
  });
}

async function commandAvailability(command, args, env = {}) {
  try {
    const result = await runCommand(command, args, { env, timeoutMs: 5000 });
    const detail = firstLine(result.stdout || result.stderr) || 'Available.';
    return { available: true, detail: detail.slice(0, 200) };
  } catch (error) {
    return { available: false, detail: publicError(error) };
  }
}

function commandResult(result) {
  return {
    exitCode: result.code,
    stdout: result.stdout,
    stderr: result.stderr,
    outputTruncated: result.outputTruncated
  };
}

class CommandError extends Error {
  constructor(command, result) {
    super(`${basename(command)} failed${result.code === null ? '' : ` with exit code ${result.code}`}: ${firstLine(result.stderr || result.stdout) || 'no diagnostic output'}`);
    this.result = result;
  }
}

function tartCommand() {
  return process.env.APPLE_SECURITY_TART_COMMAND || 'tart';
}

function xcrunCommand() {
  return process.env.APPLE_SECURITY_XCRUN_COMMAND || '/usr/bin/xcrun';
}

function assertDarwinHost(subject) {
  if (process.platform !== 'darwin' && process.env.APPLE_SECURITY_TEST_PLATFORM !== 'darwin') {
    throw new Error(`${subject} tooling requires a macOS host.`);
  }
}

function assertNoSimulator(value) {
  const visit = (current) => {
    if (typeof current === 'string' && SIMULATOR_PATTERN.test(current)) {
      throw new Error('iOS Simulator security research is prohibited. Use an authorized physical iPhone or report that realistic iOS validation is unavailable.');
    }
    if (Array.isArray(current)) for (const item of current) visit(item);
    else if (isRecord(current)) for (const [key, item] of Object.entries(current)) {
      visit(key);
      visit(item);
    }
  };
  visit(value);
}

function canonicalDirectory(value, field) {
  const path = resolve(requiredString(value, field, 4096));
  if (!existsSync(path) || !statSync(path).isDirectory()) throw new Error(`${field} must identify an existing directory.`);
  return realpathSync(path);
}

function canonicalFile(value, field) {
  const requested = requiredString(value, field, 4096);
  if (!isAbsolute(requested)) throw new Error(`${field} must be an absolute host path.`);
  const path = resolve(requested);
  if (!existsSync(path) || !statSync(path).isFile()) throw new Error(`${field} must identify an existing regular file.`);
  return realpathSync(path);
}

function destinationFile(value, field) {
  const requested = requiredString(value, field, 4096);
  if (!isAbsolute(requested)) throw new Error(`${field} must be an absolute host path.`);
  const path = resolve(requested);
  const parent = dirname(path);
  if (!existsSync(parent) || !statSync(parent).isDirectory()) throw new Error(`${field} parent directory must exist.`);
  const canonicalParent = realpathSync(parent);
  const destination = join(canonicalParent, basename(path));
  if (existsSync(destination) && !statSync(destination).isFile()) throw new Error(`${field} must not identify a directory or special file.`);
  return destination;
}

function safeGuestFilePath(value, field) {
  const path = requiredString(value, field, 4096);
  if (!path.startsWith('/') || path === '/' || path.endsWith('/')) throw new Error(`${field} must be an absolute guest file path.`);
  if (/\r|\n/u.test(path)) throw new Error(`${field} must contain exactly one line.`);
  return path;
}

function temporaryGuestPath(path) {
  const temporary = join(dirname(path), `.${safeFilename(basename(path))}.beale-upload-${randomUUID()}`);
  if (temporary.length > 4096) throw new Error('guestPath is too long for a bounded staging path.');
  return temporary;
}

function removeFileIfPresent(path) {
  try {
    if (existsSync(path)) unlinkSync(path);
  } catch {
    // Only random staging files created by the current operation reach this cleanup path.
  }
}

function fileModeText(mode) {
  return (mode & 0o777).toString(8).padStart(3, '0');
}

function containedExistingFile(root, relativePath) {
  const candidate = resolve(root, relativePath);
  assertContained(root, candidate, relativePath);
  if (!existsSync(candidate) || !statSync(candidate).isFile()) return null;
  const canonical = realpathSync(candidate);
  assertContained(realpathSync(root), canonical, relativePath);
  return canonical;
}

function assertContained(root, candidate, field) {
  const relation = relative(resolve(root), resolve(candidate));
  if (relation === '..' || relation.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) || resolve(candidate) === resolve(root)) {
    if (resolve(candidate) !== resolve(root) || field !== 'runId') throw new Error(`${field} escapes its allowed root.`);
  }
}

function isDirectory(path) {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

function isPidRunning(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function sha256File(path) {
  return new Promise((resolvePromise, rejectPromise) => {
    const hash = createHash('sha256');
    const stream = createReadStream(path);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('error', rejectPromise);
    stream.on('end', () => resolvePromise(hash.digest('hex')));
  });
}

function safeVmName(value) {
  const name = requiredString(value, 'vmName', 128);
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/u.test(name)) throw new Error('vmName contains unsupported characters.');
  return name;
}

function tartNetworkMode(value) {
  if (value === undefined || value === 'shared') return 'shared';
  if (value === 'host-only') return 'host-only';
  throw new Error('networkMode must be either shared or host-only.');
}

function safeArgv(value) {
  if (!Array.isArray(value) || value.length < 1 || value.length > 128) throw new Error('argv must contain 1 to 128 arguments.');
  return value.map((item, index) => requiredArgument(item, `argv[${index}]`));
}

function safeOptionalArgv(value) {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 128) throw new Error('arguments must contain at most 128 items.');
  return value.map((item, index) => requiredArgument(item, `arguments[${index}]`, true));
}

function requiredArgument(value, field, allowEmpty = false) {
  if (typeof value !== 'string' || value.length > 4096 || (!allowEmpty && !value)) throw new Error(`${field} must be a bounded string.`);
  if (value.includes('\0')) throw new Error(`${field} contains a NUL byte.`);
  return value;
}

function safeEnvironment(value) {
  if (!isRecord(value) || Object.keys(value).length > 64) throw new Error('environment must be an object with at most 64 entries.');
  return Object.fromEntries(Object.entries(value).map(([key, item]) => {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/u.test(key)) throw new Error(`Invalid environment key: ${key}`);
    if (typeof item !== 'string' || item.length > 4096 || item.includes('\0')) throw new Error(`Invalid environment value for ${key}.`);
    return [key, item];
  }));
}

function parseJson(value, label) {
  try {
    return JSON.parse(value);
  } catch {
    throw new Error(`${label} was not valid JSON.`);
  }
}

function flattenScalarEntries(value, prefix = '', depth = 0) {
  if (depth > 8) return [];
  if (Array.isArray(value)) return value.flatMap((item, index) => flattenScalarEntries(item, `${prefix}[${index}]`, depth + 1));
  if (!isRecord(value)) return [[prefix, value]];
  return Object.entries(value).flatMap(([key, item]) => flattenScalarEntries(item, prefix ? `${prefix}.${key}` : key, depth + 1));
}

function scalarFieldMap(value) {
  return new Map(flattenScalarEntries(value).filter(([, item]) => ['string', 'number', 'boolean'].includes(typeof item)));
}

function pickField(fields, candidates) {
  for (const candidate of candidates) {
    if (fields.has(candidate)) return fields.get(candidate);
    const suffix = [...fields.entries()].find(([key]) => key.endsWith(`.${candidate}`));
    if (suffix) return suffix[1];
  }
  return null;
}

function dedupeBy(values, keyFn) {
  const seen = new Set();
  return values.filter((value) => {
    const key = keyFn(value);
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function firstString(object, keys) {
  for (const key of keys) if (typeof object[key] === 'string') return object[key];
  return null;
}

function firstBoolean(object, keys) {
  for (const key of keys) if (typeof object[key] === 'boolean') return object[key];
  return null;
}

function firstNumber(object, keys) {
  for (const key of keys) if (typeof object[key] === 'number' && Number.isFinite(object[key])) return object[key];
  return null;
}

function firstLine(value) {
  return String(value ?? '').split(/\r?\n/u).find((line) => line.trim())?.trim() ?? '';
}

function firstLines(value, maximum) {
  return String(value ?? '').split(/\r?\n/u).map((line) => line.trim()).filter(Boolean).slice(0, maximum).join(' | ');
}

function safeFilename(value) {
  return value.replace(/[^A-Za-z0-9._-]/gu, '_').slice(0, 128);
}

function requiredString(value, field, maxLength) {
  if (typeof value !== 'string' || !value.trim() || value.trim().length > maxLength) {
    throw new Error(`${field} must be a non-empty string of at most ${maxLength} characters.`);
  }
  if (value.includes('\0')) throw new Error(`${field} contains a NUL byte.`);
  return value.trim();
}

function requiredSingleLine(value, field, maxLength) {
  const result = requiredString(value, field, maxLength);
  if (/\r|\n/u.test(result)) throw new Error(`${field} must contain exactly one line.`);
  return result;
}

function boundedInteger(value, minimum, maximum, fallback) {
  return Number.isInteger(value) && value >= minimum && value <= maximum ? value : fallback;
}

function publicError(error, args = {}) {
  let message = errorMessage(error);
  const sensitive = [
    ...deviceRefs.values(),
    ...flattenScalarEntries(args).map(([, value]) => value).filter((value) => typeof value === 'string' && value.startsWith('/'))
  ];
  for (const value of sensitive) message = message.split(value).join('<redacted>');
  message = message.replace(/\/Users\/[^/\s]+/gu, '/Users/<redacted>');
  return message.slice(0, 2000);
}

function textResult(value) {
  return { content: [{ type: 'text', text: JSON.stringify(value, null, 2) }] };
}

function sendToolError(id, message) {
  sendResult(id, { isError: true, content: [{ type: 'text', text: message }] });
}

function sendResult(id, result) {
  send({ jsonrpc: '2.0', id, result });
}

function sendError(id, code, message) {
  send({ jsonrpc: '2.0', id, error: { code, message } });
}

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

function isRecord(value) {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function stringField(maxLength) {
  return { type: 'string', minLength: 1, maxLength };
}

function integerField(minimum, maximum, defaultValue) {
  return { type: 'integer', minimum, maximum, default: defaultValue };
}

function deviceRefField() {
  return { type: 'string', pattern: '^device_[a-f0-9]{20}$' };
}

function runIdField() {
  return { type: 'string', pattern: '^darwin_[0-9a-f-]{36}$' };
}

function objectSchema(properties, required = []) {
  return { type: 'object', properties, ...(required.length ? { required } : {}), additionalProperties: false };
}

function toolAnnotation(actionClasses, sideEffects, requiredPermissions, confirmation) {
  return {
    readOnlyHint: sideEffects === 'read',
    destructiveHint: sideEffects === 'write',
    openWorldHint: false,
    'beale.io/tool': { actionClasses, sideEffects, requiredPermissions, confirmation }
  };
}
