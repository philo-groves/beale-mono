import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { access, mkdtemp, mkdir, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import test from "node:test";
import { promisify } from "node:util";

import {
  createAnalysisTool,
  createCodeIntelligenceTools,
  createDefaultBuiltInToolFamily,
  decodeWslListOutput,
  createExperimentTool,
  createPriorArtSearchTool,
  createRepositoryHistoryTool,
  createRepositorySearchTool,
  RepositoryResearchSession,
  createResearchStorageLayout,
  createResearchToolRegistry,
  createShellTool,
  createToolResultMessage,
  createStorageListTool,
  createStructuredFileReadTool,
  createSynthesisTool,
  ensureResearchStorageLayout,
  modelToolResultDetails,
  projectModelToolResult,
  registerResearchStorageArtifact,
  resolveWindowsPowerShellExecutable,
  translateWindowsPathsInCommand,
  windowsPathToWsl,
} from "../packages/research-agent/dist/index.js";

const allowShell = async (request) => approvedAuthorization(request);
const execFileAsync = promisify(execFile);

function approvedAuthorization(request) {
  return {
    approvalRequestId: "fixture_" + request.actionId,
    actionId: request.actionId,
    mode: "danger",
    decision: "approved",
    source: "danger",
    reason: "Danger Mode test fixture.",
    command: {
      commandHash: "sha256:fixture",
      utility: request.utility,
      args: request.args,
      cwd: request.cwd,
      timeoutMs: request.timeoutMs,
      stdinPresent: request.stdin !== undefined,
      stdinBytes: request.stdin?.length ?? 0,
    },
  };
}

test("shell tool enforces disabled utilities before spawning and captures argv output", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-tool-"));
  const optionsPath = join(root, "shell-options.json");
  const protectedDirectory = join(root, "protected-core");
  const protectedChildDirectory = join(protectedDirectory, "nested");
  const disposableDirectory = join(root, "build-output");
  await mkdir(protectedChildDirectory, { recursive: true });
  await mkdir(disposableDirectory);
  await writeFile(optionsPath, JSON.stringify({
    schemaVersion: 1,
    defaultConcurrency: 2,
    utilities: { sudo: 0 },
    leaseDirectory: join(root, "leases"),
  }));

  try {
    const shellTool = createShellTool({
      workspaceRoot: root,
      shellOptionsPath: optionsPath,
      protectedDirectories: [protectedDirectory],
      authorize: allowShell,
    });
    assert.equal(shellTool.parameters.type, "object");
    assert.equal("anyOf" in shellTool.parameters, false);
    assert.equal("oneOf" in shellTool.parameters, false);
    const registry = createResearchToolRegistry([shellTool]);
    const missingInvocation = await registry.execute({
      id: "shell_missing_invocation",
      actionClass: "inspect",
      toolName: "shell.run",
      input: {},
    });
    const conflictingInvocation = await registry.execute({
      id: "shell_conflicting_invocation",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { command: "echo safe", utility: "printf" },
    });
    const disabled = await registry.execute({
      id: "shell_disabled",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "sudo", args: ["true"] },
    });
    const completed = await registry.execute({
      id: "shell_completed",
      actionClass: "inspect",
      toolName: "shell.run",
      input: {
        utility: "node",
        args: ["-e", "process.stdout.write(process.argv[1])", "argv-safe"],
      },
    });
    const pathCompleted = await registry.execute({
      id: "shell_path_completed",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { utility: process.execPath, args: ["-e", "process.stdout.write('path-safe')"] },
    });
    const commandCompleted = await registry.execute({
      id: "shell_command_completed",
      actionClass: "inspect",
      toolName: "shell.run",
      input: {
        command: process.platform === "win32"
          ? "Write-Output command-one; Write-Output command-two"
          : "printf command-one && printf command-two",
      },
    });
    const homeReference = await registry.execute({
      id: "shell_home_reference",
      actionClass: "inspect",
      toolName: "shell.run",
      input: {
        utility: "node",
        args: ["-e", "process.stdout.write(process.argv[1])", "${HOME:-/}"],
      },
    });
    const homeAssignment = await registry.execute({
      id: "shell_home_assignment",
      actionClass: "experiment",
      toolName: "shell.run",
      input: {
        utility: "node",
        args: ["-e", "process.stdout.write(process.argv[2])", "HOME=/tmp", "safe"],
      },
    });
    const environment = await registry.execute({
      id: "shell_environment",
      actionClass: "inspect",
      toolName: "shell.run",
      input: {
        utility: "node",
        args: [
          "-e",
          "process.stdout.write(JSON.stringify(Object.fromEntries(Object.entries(process.env).filter(([name]) => ['HOME', 'CODEX_HOME', 'HOMEDRIVE', 'HOMEPATH', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA'].includes(name.toUpperCase())))))",
        ],
      },
    });
    assert.equal(missingInvocation.result.status, "error");
    assert.match(missingInvocation.result.error.message, /requires command or utility/);
    assert.equal(conflictingInvocation.result.status, "error");
    assert.match(conflictingInvocation.result.error.message, /not both/);
    const protectedDelete = await registry.execute({
      id: "shell_protected_delete",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "rm", args: ["-rf", protectedDirectory] },
    });
    const workspaceDelete = await registry.execute({
      id: "shell_workspace_delete",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "rm", args: ["-rf", root] },
    });
    const protectedChildDelete = await registry.execute({
      id: "shell_protected_child_delete",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "rmdir", args: [protectedChildDirectory] },
    });
    const findDelete = await registry.execute({
      id: "shell_find_delete",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "find", args: [protectedDirectory, "-depth", "-delete"] },
    });
    const gitClean = await registry.execute({
      id: "shell_git_clean",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "git", args: ["-C", root, "clean", "-fdx"] },
    });
    const disposableDelete = await registry.execute({
      id: "shell_disposable_delete",
      actionClass: "experiment",
      toolName: "shell.run",
      input: process.platform === "win32"
        ? {
            utility: process.execPath,
            args: [
              "-e",
              "require('node:fs').rmSync(process.argv[1], { recursive: true, force: true })",
              disposableDirectory,
            ],
          }
        : { utility: "rm", args: ["-rf", disposableDirectory] },
    });

    assert.equal(disabled.result.status, "error");
    assert.match(disabled.result.error.message, /disabled by the harness-wide/);
    assert.equal(completed.result.status, "complete");
    assert.equal(completed.result.output.stdout, "argv-safe");
    assert.equal(completed.result.output.cwd, root);
    assert.equal(pathCompleted.result.status, "complete");
    assert.equal(pathCompleted.result.output.stdout, "path-safe");
    assert.equal(commandCompleted.result.status, "complete");
    assert.match(commandCompleted.result.output.stdout, /command-one/);
    assert.match(commandCompleted.result.output.stdout, /command-two/);
    assert.equal(homeReference.result.status, "complete");
    assert.match(homeReference.result.output.stdout, /HOME/);
    assert.equal(homeAssignment.result.status, "complete");
    assert.equal(homeAssignment.result.output.stdout, "safe");
    assert.equal(environment.result.status, "complete");
    const homeEnvironment = JSON.parse(environment.result.output.stdout);
    const homeNames = new Set(["HOME", "CODEX_HOME", "HOMEDRIVE", "HOMEPATH", "USERPROFILE", "APPDATA", "LOCALAPPDATA"]);
    const expectedHomeEnvironment = Object.fromEntries(
      Object.entries(process.env).filter(([name, value]) => value !== undefined && homeNames.has(name.toUpperCase())),
    );
    assert.deepEqual(homeEnvironment, expectedHomeEnvironment);
    assert.equal(protectedDelete.result.status, "error");
    assert.match(protectedDelete.result.error.message, /Folder delete guard blocked rm/);
    assert.equal(workspaceDelete.result.status, "error");
    assert.match(workspaceDelete.result.error.message, /Folder delete guard blocked rm/);
    assert.equal(protectedChildDelete.result.status, "error");
    assert.match(protectedChildDelete.result.error.message, /Folder delete guard blocked rmdir/);
    assert.equal(findDelete.result.status, "error");
    assert.match(findDelete.result.error.message, /Folder delete guard blocked find/);
    assert.equal(gitClean.result.status, "error");
    assert.match(gitClean.result.error.message, /Folder delete guard blocked git/);
    assert.equal(disposableDelete.result.status, "complete");
    await access(protectedDirectory);
    await assert.rejects(access(disposableDirectory));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("shell tool exposes PowerShell and WSL guidance only on Windows", () => {
  const macTool = createShellTool({ workspaceRoot: "/tmp/research", platform: "darwin" });
  const windowsTool = createShellTool({ workspaceRoot: "C:\\research", platform: "win32" });
  const macGuidance = JSON.stringify({
    description: macTool.descriptor.description,
    parameters: macTool.parameters,
  });
  const windowsGuidance = JSON.stringify({
    description: windowsTool.descriptor.description,
    parameters: windowsTool.parameters,
  });

  assert.doesNotMatch(macGuidance, /powershell|pwsh|\bwsl\b/iu);
  assert.deepEqual(macTool.parameters.properties.runtime.enum, ["host"]);
  assert.match(windowsGuidance, /powershell/iu);
  assert.match(windowsGuidance, /\bwsl\b/iu);
  assert.deepEqual(windowsTool.parameters.properties.runtime.enum, ["host", "wsl"]);
});

test("shell tool defaults Windows command form to PowerShell and keeps explicit WSL path translation", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-wsl-"));
  const nested = join(root, "folder with space");
  const target = join(nested, "fixture.go");
  const requests = [];
  const authorize = async (request) => {
    requests.push(request);
    return {
      ...approvedAuthorization(request),
      mode: "manual_approval",
      decision: "denied",
      source: "human",
      reason: "Fixture denial.",
    };
  };
  try {
    const registry = createResearchToolRegistry([
      createShellTool({
        workspaceRoot: root,
        platform: "win32",
        windowsPowerShell: { executable: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe" },
        wsl: {
          executable: "wsl.exe",
          listDistributions: () => ["Ubuntu", "docker-desktop"],
        },
        authorize,
      }),
    ]);
    await registry.execute({
      id: "shell_powershell_default",
      actionClass: "inspect",
      toolName: "shell.run",
      input: {
        command: `grep -n fixture "${target}" | head -20`,
        cwd: nested,
      },
    });
    await registry.execute({
      id: "shell_wsl_explicit",
      actionClass: "inspect",
      toolName: "shell.run",
      input: {
        command: `grep -n fixture "${target}" | head -20`,
        cwd: nested,
        runtime: "wsl",
      },
    });

    assert.equal(requests.length, 2);
    assert.equal(requests[0].utility, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    assert.deepEqual(requests[0].args, [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      `grep -n fixture "${target}" | head -20`,
    ]);
    assert.equal(requests[0].cwd, nested);
    assert.equal(requests[1].utility, "wsl.exe");
    assert.deepEqual(requests[1].args.slice(0, 7), [
      "--distribution",
      "Ubuntu",
      "--cd",
      windowsPathToWsl(nested, "Ubuntu"),
      "--exec",
      "/bin/sh",
      "-lc",
    ]);
    assert.equal(
      requests[1].args[7],
      `grep -n fixture "${windowsPathToWsl(target, "Ubuntu")}" | head -20`,
    );
    assert.equal(requests[1].cwd, root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("shell tool keeps direct utilities native and honors explicit runtime selection", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-runtime-"));
  const requests = [];
  const authorize = async (request) => {
    requests.push(request);
    return {
      ...approvedAuthorization(request),
      mode: "manual_approval",
      decision: "denied",
      source: "human",
      reason: "Fixture denial.",
    };
  };
  try {
    const registry = createResearchToolRegistry([
      createShellTool({
        workspaceRoot: root,
        platform: "win32",
        windowsPowerShell: { executable: "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe" },
        wsl: { listDistributions: () => ["Ubuntu"] },
        authorize,
      }),
    ]);
    await registry.execute({
      id: "shell_native_utility",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { utility: "git", args: ["status"] },
    });
    await registry.execute({
      id: "shell_explicit_host",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { command: "echo host", runtime: "host" },
    });
    await registry.execute({
      id: "shell_powershell_alias",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { utility: "pwsh", args: ["-NoProfile", "-Command", "Write-Output ready"] },
    });
    await registry.execute({
      id: "shell_explicit_wsl",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { utility: "git", args: ["-C", root, "status"], runtime: "wsl" },
    });
    const disabledWsl = await registry.execute({
      id: "shell_explicit_wsl_sudo",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "sudo", args: ["true"], runtime: "wsl" },
    });
    const guardedWsl = await registry.execute({
      id: "shell_explicit_wsl_delete",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "rm", args: ["-rf", "/etc"], runtime: "wsl" },
    });

    assert.equal(requests[0].utility, "git");
    assert.equal(requests[1].utility, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    assert.equal(requests[2].utility, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    assert.equal(requests[3].utility, "wsl.exe");
    assert.equal(requests[3].args.at(-2), windowsPathToWsl(root, "Ubuntu"));
    assert.equal(requests[3].args.at(-1), "status");
    assert.equal(disabledWsl.result.status, "error");
    assert.match(disabledWsl.result.error.message, /sudo is disabled/);
    assert.equal(guardedWsl.result.status, "error");
    assert.match(guardedWsl.result.error.message, /protected WSL directory \/etc/);
    assert.equal(requests.length, 4);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("WSL path translation preserves shell syntax and supports WSL UNC paths", () => {
  assert.equal(decodeWslListOutput(Buffer.from("\uFEFFUbuntu\r\ndocker-desktop\r\n", "utf16le")), "Ubuntu\r\ndocker-desktop\r\n");
  assert.equal(windowsPathToWsl("C:\\Research\\target repo\\file.c"), "/mnt/c/Research/target repo/file.c");
  assert.equal(windowsPathToWsl("\\\\wsl$\\Ubuntu\\home\\analyst", "Ubuntu"), "/home/analyst");
  assert.equal(windowsPathToWsl("\\\\wsl$\\Debian\\home\\analyst", "Ubuntu"), null);
  assert.equal(
    translateWindowsPathsInCommand('rg TODO "C:\\Research\\target repo" 2>/dev/null | head -20'),
    'rg TODO "/mnt/c/Research/target repo" 2>/dev/null | head -20',
  );
});

test("Windows PowerShell detection prefers PowerShell 7 and falls back to the inbox system runner", () => {
  const powerShell7 = "C:\\Program Files\\PowerShell\\7\\pwsh.exe";
  const inboxPowerShell = "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
  const environment = { ProgramFiles: "C:\\Program Files", SystemRoot: "C:\\Windows" };

  assert.equal(resolveWindowsPowerShellExecutable("win32", {
    environment,
    pathExists: (path) => path === powerShell7,
  }), powerShell7);
  assert.equal(resolveWindowsPowerShellExecutable("win32", {
    environment,
    pathExists: (path) => path === inboxPowerShell,
  }), inboxPowerShell);
  assert.equal(resolveWindowsPowerShellExecutable("linux", {
    environment,
    pathExists: () => true,
  }), null);
});

test("Windows shell execution routes default commands and bare pwsh through the detected host runner", {
  skip: process.platform !== "win32",
}, async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-powershell-"));
  try {
    const registry = createResearchToolRegistry([createShellTool({ workspaceRoot: root, authorize: allowShell })]);
    const command = await registry.execute({
      id: "shell_windows_powershell_default",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { command: "Write-Output 'default-powershell-ready'" },
    });
    const alias = await registry.execute({
      id: "shell_windows_powershell_alias",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { utility: "pwsh", args: ["-NoProfile", "-Command", "Write-Output 'runbook-powershell-ready'"] },
    });

    assert.equal(command.result.status, "complete");
    assert.equal(command.result.output.stdout.trim(), "default-powershell-ready");
    assert.match(command.result.output.utility, /(?:pwsh|powershell)\.exe$/iu);
    assert.equal(alias.result.status, "complete");
    assert.equal(alias.result.output.stdout.trim(), "runbook-powershell-ready");
    assert.match(alias.result.output.utility, /(?:pwsh|powershell)\.exe$/iu);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("shell tool serializes the same utility across tool instances", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-concurrency-"));
  const optionsPath = join(root, "shell-options.json");
  await writeFile(optionsPath, JSON.stringify({
    schemaVersion: 1,
    defaultConcurrency: 4,
    utilities: { node: 1 },
    leaseDirectory: join(root, "leases"),
  }));

  try {
    const first = createResearchToolRegistry([
      createShellTool({ workspaceRoot: root, shellOptionsPath: optionsPath, authorize: allowShell }),
    ]);
    const second = createResearchToolRegistry([
      createShellTool({ workspaceRoot: root, shellOptionsPath: optionsPath, authorize: allowShell }),
    ]);
    const startedAt = Date.now();
    const results = await Promise.all([
      first.execute({
        id: "node_1",
        actionClass: "experiment",
        toolName: "shell.run",
        input: { utility: "node", args: ["-e", "setTimeout(() => {}, 150)"] },
      }),
      second.execute({
        id: "node_2",
        actionClass: "experiment",
        toolName: "shell.run",
        input: { utility: "node", args: ["-e", "setTimeout(() => {}, 150)"] },
      }),
    ]);

    assert.ok(results.every((result) => result.result.status === "complete"));
    assert.ok(Date.now() - startedAt >= 250, "same-utility calls should not overlap at concurrency 1");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("shell tool terminates descendant processes when a command times out", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-timeout-"));
  try {
    const registry = createResearchToolRegistry([
      createShellTool({ workspaceRoot: root, authorize: allowShell }),
    ]);
    const startedAt = Date.now();
    const parentScript = [
      "const { spawn } = require('node:child_process');",
      "const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });",
      "process.stdout.write(`${child.pid}\\n`);",
      "setInterval(() => {}, 1000);",
    ].join("");
    const timedOut = await registry.execute({
      id: "shell_timeout_tree",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "node", args: ["-e", parentScript], timeoutMs: 250 },
    });

    assert.equal(timedOut.result.status, "error");
    assert.match(timedOut.result.error.message, /timed out/);
    assert.ok(Date.now() - startedAt < 5_000, "timed-out descendant must not hold the tool output pipes open");
    const descendantPid = Number.parseInt(timedOut.result.output.stdout.trim(), 10);
    assert.ok(Number.isInteger(descendantPid));
    await assertProcessExited(descendantPid);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("shell tool blocks denied commands before spawn and keeps hard guards ahead of authorization", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-denied-"));
  const marker = join(root, "spawned.txt");
  const protectedDirectory = join(root, "protected");
  await mkdir(protectedDirectory);
  const requests = [];
  const authorize = async (request) => {
    requests.push(request);
    return {
      ...approvedAuthorization(request),
      mode: "manual_approval",
      decision: "denied",
      source: "human",
      reason: "Fixture denial.",
    };
  };
  try {
    const registry = createResearchToolRegistry([
      createShellTool({
        workspaceRoot: root,
        protectedDirectories: [protectedDirectory],
        authorize,
      }),
    ]);
    const denied = await registry.execute({
      id: "shell_denied_before_spawn",
      actionClass: "experiment",
      toolName: "shell.run",
      input: {
        utility: "node",
        args: [
          "-e",
          "require('node:fs').writeFileSync(" + JSON.stringify(marker) + ", 'spawned')",
        ],
        cwd: ".",
        stdin: "token=secret-value",
        timeoutMs: 1_000,
      },
    });
    assert.equal(denied.result.status, "blocked");
    assert.match(denied.result.summary, /denied by Manual Approval/);
    await assert.rejects(access(marker));
    assert.equal(requests.length, 1);
    assert.equal(requests[0].cwd, root);
    assert.equal(requests[0].stdin, "token=secret-value");

    const hardGuarded = await registry.execute({
      id: "shell_guard_before_authorizer",
      actionClass: "experiment",
      toolName: "shell.run",
      input: { utility: "rm", args: ["-rf", protectedDirectory] },
    });
    assert.equal(hardGuarded.result.status, "error");
    assert.equal(requests.length, 1);
    await access(protectedDirectory);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("shell capture events and tool results omit stdin and redact credential argv values", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-redaction-"));
  const secrets = [
    "raw-stdin-secret",
    "password-argv-secret",
    "token-argv-secret",
    "header-argv-secret",
    "user-password-secret",
    "cookie-pair-secret",
    "cookie-short-secret",
    "cookie-header-secret",
  ];
  try {
    const registry = createResearchToolRegistry([
      createShellTool({ workspaceRoot: root, authorize: allowShell }),
    ]);
    const record = await registry.execute({
      id: "shell_sanitized_transport",
      actionClass: "experiment",
      toolName: "shell.run",
      input: {
        utility: "node",
        args: [
          "-e",
          "",
          "--",
          "--password",
          secrets[1],
          "--token",
          secrets[2],
          "-H",
          `Authorization: Basic ${secrets[3]}`,
          "--user",
          `researcher:${secrets[4]}`,
          "--cookie",
          `session=${secrets[5]}`,
          "-b",
          secrets[6],
          "--header",
          `Cookie: session=${secrets[7]}`,
        ],
        cwd: ".",
        stdin: secrets[0],
        timeoutMs: 1_000,
      },
    });
    assert.equal(record.result.status, "complete");
    const toolResult = createToolResultMessage(record.result, record.action.id, "shell_run");
    const captured = JSON.stringify({ record, toolResult });
    for (const secret of secrets) assert.doesNotMatch(captured, new RegExp(secret));

    for (const event of record.events) {
      const normalized = event.payload.normalizedInputs;
      assert.equal("stdin" in normalized, false);
      assert.equal(normalized.stdinPresent, true);
      assert.equal(normalized.stdinBytes, Buffer.byteLength(secrets[0]));
      assert.match(normalized.stdinHash, /^sha256:/);
      assert.equal(normalized.timeoutMs, 1_000);
    }
    assert.deepEqual(record.action.input.args.slice(3), [
      "--password",
      "[REDACTED]",
      "--token",
      "[REDACTED]",
      "-H",
      "Authorization: [REDACTED]",
      "--user",
      "[REDACTED]",
      "--cookie",
      "[REDACTED]",
      "-b",
      "[REDACTED]",
      "--header",
      "Cookie: [REDACTED]",
    ]);
    assert.deepEqual(record.result.output.args, record.action.input.args);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("model tool results retain readable output separately from bounded metadata", () => {
  const result = {
    action: {
      id: "bounded_details",
      actionClass: "inspect",
      toolName: "fixture.inspect",
      input: { marker: "action-input-must-not-be-retained" },
    },
    status: "error",
    startedAt: "2026-08-05T12:00:00.000Z",
    completedAt: "2026-08-05T12:00:01.000Z",
    summary: "Fixture inspection failed.",
    output: { marker: "full-output-remains-in-content" },
    rawOutputRef: "raw://fixture-output",
    artifactRefs: [
      {
        id: "artifact_fixture",
        kind: "inspection",
        uri: "artifact://fixture",
      },
    ],
    followUpActions: ["Inspect the captured artifact."],
    error: { message: "Fixture failure." },
  };
  const expectedDetails = {
    status: "error",
    summary: "Fixture inspection failed.",
    rawOutputRef: "raw://fixture-output",
    artifactRefs: result.artifactRefs,
    followUpActions: result.followUpActions,
    error: { message: "Fixture failure." },
  };

  assert.deepEqual(modelToolResultDetails(result), expectedDetails);

  const projection = projectModelToolResult(result);
  assert.deepEqual(projection.details, expectedDetails);
  assert.equal(projection.isError, true);
  assert.match(projection.content[0].text, /full-output-remains-in-content/);

  const message = createToolResultMessage(
    result,
    "tool_call_bounded_details",
    "fixture_inspect",
  );
  assert.deepEqual(message.details, expectedDetails);
  assert.equal("action" in message.details, false);
  assert.equal("output" in message.details, false);
  assert.equal("startedAt" in message.details, false);
  assert.equal("completedAt" in message.details, false);
  assert.match(message.content[0].text, /full-output-remains-in-content/);

  const boundedMemoryProjection = projectModelToolResult({
    ...result,
    action: { ...result.action, toolName: "memory.search" },
    status: "complete",
    output: { text: "x".repeat(20_000) },
    error: undefined,
  });
  assert.match(boundedMemoryProjection.content[0].text, /Tool result truncated for model context/);
  assert.ok(boundedMemoryProjection.content[0].text.length < 13_000);
});

test("tool runtime budget aborts a pending approval before any later spawn", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-budget-"));
  const marker = join(root, "late-spawn.txt");
  let approve;
  const authorize = (request) => new Promise((resolveApproval) => {
    approve = () => resolveApproval(approvedAuthorization(request));
  });
  try {
    const registry = createResearchToolRegistry([
      createShellTool({ workspaceRoot: root, authorize }),
    ]);
    const result = await registry.execute({
      id: "shell_late_approval",
      actionClass: "experiment",
      toolName: "shell.run",
      input: {
        utility: "node",
        args: [
          "-e",
          "require('node:fs').writeFileSync(" + JSON.stringify(marker) + ", 'spawned')",
        ],
      },
    }, {
      governance: { maxRuntimeMs: 25 },
    });
    assert.equal(result.result.status, "blocked");
    assert.match(result.result.summary, /runtime budget exceeded/);
    assert.deepEqual(result.events.map((event) => event.kind), ["tool.requested", "tool.observed"]);
    assert.equal(result.events.at(-1).payload.status, "blocked");
    assert.equal(typeof approve, "function");
    approve();
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 75));
    await assert.rejects(access(marker));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("repository search finds bounded local source matches", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-repo-search-"));
  await mkdir(join(root, "Src"));
  await mkdir(join(root, ".beale", "memory"), { recursive: true });
  await mkdir(join(root, ".beale"), { recursive: true });
  await writeFile(
    join(root, "Src", "parse.c"),
    "static void parse_context_save(void) {}\nparse_context_save();\n",
  );
  await writeFile(
    join(root, ".beale", "memory", "memory.sqlite-wal"),
    "parse_context_save stale internal memory hit\n",
  );
  await writeFile(
    join(root, ".beale", "beale.sqlite-wal"),
    "parse_context_save stale interface state hit\n",
  );
  await writeFile(join(root, "README.md"), "no parser symbol here\n");
  await execFileAsync("git", ["init", "--quiet", root]);
  await execFileAsync("git", ["-C", root, "add", "Src/parse.c", "README.md"]);

  try {
    const tool = createRepositorySearchTool({
      root,
      maxResults: 1,
      maxFileBytes: 1024,
    });
    const result = await createResearchToolRegistry([tool]).execute({
      id: "search_1",
      actionClass: "search",
      toolName: "repository.search",
      input: {
        query: "parse_context_save",
      },
    });

    assert.equal(result.result.status, "complete");
    assert.equal(result.result.output.matches.length, 1);
    assert.equal(result.result.output.matches[0].path, "Src/parse.c");
    assert.ok(
      result.result.output.matches.every(
        (match) =>
          !match.path.startsWith(".beale/") &&
          !match.path.startsWith(".beale/"),
      ),
    );
    assert.equal(result.result.output.matches[0].line, 1);
    assert.equal(tool.descriptor.sideEffects, "read");
    assert.equal(tool.descriptor.requiredPermissions[0], "filesystem:read");
    assert.deepEqual(tool.descriptor.actionClasses, ["search", "inspect"]);

    const modelProjection = projectModelToolResult(result.result);
    const modelPayload = JSON.parse(modelProjection.content[0].text);
    assert.equal(modelPayload.output.query, "parse_context_save");
    assert.equal(modelPayload.output.matches[0].path, "Src/parse.c");
    assert.equal(modelPayload.output.searchedRootCount, 1);
    assert.equal("roots" in modelPayload.output, false);
    assert.equal("availableRoots" in modelPayload.output, false);
    assert.equal("attemptedRoots" in modelPayload.output, false);

    const observed = result.events.find((event) => event.kind === "tool.observed");
    assert.ok(observed);
    assert.deepEqual(observed.payload.result.availableRoots, result.result.output.availableRoots);
    assert.ok(observed.payload.fullResultCharacters > observed.payload.modelVisibleResultCharacters);
    assert.equal(
      observed.payload.modelResultCharactersRemoved,
      observed.payload.fullResultCharacters - observed.payload.modelVisibleResultCharacters,
    );

    const inspectResult = await createResearchToolRegistry([tool]).execute({
      id: "inspect_search_1",
      actionClass: "inspect",
      toolName: "repository.search",
      input: {
        query: "parse_context_save",
      },
    });

    assert.equal(inspectResult.result.status, "complete");
    assert.equal(inspectResult.result.output.matches[0].path, "Src/parse.c");
  } finally {
    await rm(root, {
      recursive: true,
      force: true,
    });
  }
});

test("repository first touch is emitted once per canonical repository revision", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-repo-touch-"));
  await writeFile(join(root, "parser.c"), "int parser_boundary = 1;\n");
  await execFileAsync("git", ["init", "--quiet", root]);
  await execFileAsync("git", ["-C", root, "add", "parser.c"]);
  await execFileAsync("git", ["-C", root, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.com", "commit", "--quiet", "-m", "initial parser"]);
  try {
    const researchSession = new RepositoryResearchSession();
    const tool = createRepositorySearchTool({ root, researchSession });
    const registry = createResearchToolRegistry([tool]);
    const first = await registry.execute({
      id: "first_touch_search",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "parser_boundary" },
    });
    assert.equal(first.result.status, "complete");
    assert.equal(first.result.output.repositoryFirstTouches.length, 1);
    assert.equal(first.result.output.repositoryFirstTouches[0].repository.root, await realpath(root));
    assert.equal(first.result.output.repositoryFirstTouches[0].repository.shallow, false);
    assert.match(first.result.output.repositoryFirstTouches[0].reminder.join(" "), /public CVEs/);
    assert.match(first.result.output.repositoryFirstTouches[0].reminder.join(" "), /release notes/);
    assert.match(first.result.output.repositoryFirstTouches[0].reminder.join(" "), /Apple Open Source/);

    const second = await registry.execute({
      id: "second_touch_search",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "parser_boundary" },
    });
    assert.deepEqual(second.result.output.repositoryFirstTouches, []);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("repository first touch waits for scope-relevance review and can retry a denied trigger", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-repo-reviewed-touch-"));
  await writeFile(join(root, "service.c"), "int service_boundary = 1;\n");
  await execFileAsync("git", ["init", "--quiet", root]);
  await execFileAsync("git", ["-C", root, "add", "service.c"]);
  await execFileAsync("git", ["-C", root, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.com", "commit", "--quiet", "-m", "initial service"]);
  let reviews = 0;
  try {
    const researchSession = new RepositoryResearchSession({
      beforeFirstTouch: async () => {
        reviews += 1;
        return reviews === 1
          ? { approved: false, firstTouch: false, details: { decision: "not_relevant" } }
          : { approved: true, firstTouch: true, details: { decision: "relevant", source: "auto_review" } };
      },
    });
    const registry = createResearchToolRegistry([createRepositorySearchTool({ root, researchSession })]);
    const denied = await registry.execute({
      id: "reviewed_touch_denied",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "service_boundary" },
    });
    assert.deepEqual(denied.result.output.repositoryFirstTouches, []);
    const approved = await registry.execute({
      id: "reviewed_touch_approved",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "service_boundary" },
    });
    assert.equal(approved.result.output.repositoryFirstTouches.length, 1);
    assert.equal(approved.result.output.repositoryFirstTouches[0].scopeReview.source, "auto_review");
    const repeated = await registry.execute({
      id: "reviewed_touch_repeated",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "service_boundary" },
    });
    assert.deepEqual(repeated.result.output.repositoryFirstTouches, []);
    assert.equal(reviews, 2);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("repository history exposes provenance, literal fix history, and blame", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-repo-history-"));
  await writeFile(join(root, "parser.c"), "int validate = 0;\n");
  await execFileAsync("git", ["init", "--quiet", root]);
  await execFileAsync("git", ["-C", root, "add", "parser.c"]);
  await execFileAsync("git", ["-C", root, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.com", "commit", "--quiet", "-m", "initial parser"]);
  await writeFile(join(root, "parser.c"), "int validate = 1; /* reject malformed length */\n");
  await execFileAsync("git", ["-C", root, "add", "parser.c"]);
  await execFileAsync("git", ["-C", root, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.com", "commit", "--quiet", "-m", "harden malformed length"]);
  try {
    const tool = createRepositoryHistoryTool({ roots: [root] });
    const registry = createResearchToolRegistry([tool]);
    const overview = await registry.execute({
      id: "history_overview",
      actionClass: "inspect",
      toolName: "repository.history",
      input: { operation: "overview" },
    });
    assert.equal(overview.result.status, "complete");
    assert.equal(overview.result.output.repository.shallow, false);
    assert.equal(overview.result.output.recentCommits[0].subject, "harden malformed length");
    assert.equal(overview.result.output.repositoryFirstTouch.firstTouch, true);

    const changes = await registry.execute({
      id: "history_changes",
      actionClass: "search",
      toolName: "repository.history",
      input: { operation: "search_changes", query: "reject malformed length", path: "parser.c" },
    });
    assert.match(changes.result.output.changes, /harden malformed length/);
    assert.match(changes.result.output.changes, /reject malformed length/);

    const blame = await registry.execute({
      id: "history_blame",
      actionClass: "inspect",
      toolName: "repository.history",
      input: { operation: "blame", path: "parser.c", startLine: 1, endLine: 1 },
    });
    assert.match(blame.result.output.blame, /summary harden malformed length/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("prior art search normalizes public advisory results with source URLs", async () => {
  const fetchFixture = async (url) => {
    assert.match(String(url), /services\.nvd\.nist\.gov/);
    return new Response(JSON.stringify({
      vulnerabilities: [{
        cve: {
          id: "CVE-2026-1234",
          published: "2026-01-02T00:00:00.000Z",
          lastModified: "2026-01-03T00:00:00.000Z",
          descriptions: [{ lang: "en", value: "Parser length validation bypass." }],
          references: [{ url: "https://vendor.example/advisory" }],
          configurations: [{ nodes: [{ cpeMatch: [{ criteria: "cpe:2.3:a:vendor:parser:*:*:*:*:*:*:*:*" }] }] }],
        },
      }],
    }), { status: 200, headers: { "Content-Type": "application/json" } });
  };
  const tool = createPriorArtSearchTool({ fetch: fetchFixture });
  const result = await createResearchToolRegistry([tool]).execute({
    id: "prior_art_fixture",
    actionClass: "search",
    toolName: "prior_art.search",
    input: { query: "vendor parser", sources: ["nvd"] },
  }, { governance: { allowedSideEffects: ["network"] } });
  assert.equal(result.result.status, "complete");
  assert.equal(result.result.output.disposition, "matches_found");
  assert.equal(result.result.output.records[0].id, "CVE-2026-1234");
  assert.equal(result.result.output.records[0].source, "nvd");
  assert.match(result.result.output.records[0].url, /CVE-2026-1234/);
  assert.deepEqual(result.result.output.records[0].affected, ["cpe:2.3:a:vendor:parser:*:*:*:*:*:*:*:*"]);
});

test("repository search bounds and interrupts non-Git traversal", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-repo-search-bounds-"));
  await writeFile(join(root, "first.txt"), "unrelated first file\n");
  await writeFile(join(root, "second.txt"), "unrelated second file\n");
  try {
    const boundedTool = createRepositorySearchTool({
      root,
      maxVisitedFiles: 1,
    });
    const bounded = await createResearchToolRegistry([boundedTool]).execute({
      id: "search_bounded_walk",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "not-present" },
    });
    assert.equal(bounded.result.status, "error");
    assert.match(bounded.result.summary, /stopped after inspecting 1 files/);

    const controller = new AbortController();
    controller.abort();
    const interrupted = await createResearchToolRegistry([
      createRepositorySearchTool({ root }),
    ]).execute({
      id: "search_interrupted_walk",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "not-present" },
    }, {
      signal: controller.signal,
    });
    assert.equal(interrupted.result.status, "error");
    assert.match(interrupted.result.summary, /search was interrupted/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("repository search scopes configured roots and preserves bounded partial results", async () => {
  const workspace = await mkdtemp(join(tmpdir(), "app-server-repo-search-roots-"));
  const external = await mkdtemp(join(tmpdir(), "app-server-repo-search-external-"));
  const first = join(workspace, "first-repository");
  const second = join(workspace, "second-repository");
  await mkdir(first);
  await mkdir(second);
  await writeFile(join(first, "first.txt"), "scoped needle\n");
  await writeFile(join(second, "second.txt"), "scoped needle\n");
  await writeFile(join(external, "external.txt"), "external needle\n");
  try {
    const scopedTool = createRepositorySearchTool({ roots: [first, second] });
    const scoped = await createResearchToolRegistry([scopedTool]).execute({
      id: "search_scoped_root",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "scoped needle", root: "second-repository" },
    });
    assert.equal(scoped.result.status, "complete");
    assert.equal(scoped.result.output.matches.length, 1);
    assert.equal(basename(scoped.result.output.matches[0].root), "second-repository");
    assert.equal(scoped.result.output.partial, false);
    assert.deepEqual(
      scoped.result.output.availableRoots.map((entry) => entry.label),
      ["first-repository", "second-repository"],
    );

    const nestedFile = await createResearchToolRegistry([
      createRepositorySearchTool({ roots: [workspace] }),
    ]).execute({
      id: "search_nested_file_root",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "scoped needle", root: "second-repository/second.txt" },
    });
    assert.equal(nestedFile.result.status, "complete");
    assert.equal(nestedFile.result.output.matches[0].path, "second.txt");

    const externalTool = createRepositorySearchTool({});
    const externalResult = await createResearchToolRegistry([externalTool]).execute({
      id: "search_external_root",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "external needle", root: external },
    });
    assert.equal(externalResult.result.status, "complete");
    assert.equal(externalResult.result.output.matches.length, 1);
    assert.equal(externalResult.result.output.matches[0].path, "external.txt");
    assert.deepEqual(externalResult.result.output.attemptedRoots, [await realpath(external)]);

    const partialTool = createRepositorySearchTool({
      roots: [first, second],
      maxDurationMs: 0,
    });
    const partial = await createResearchToolRegistry([partialTool]).execute({
      id: "search_partial_timeout",
      actionClass: "search",
      toolName: "repository.search",
      input: { query: "scoped needle" },
    });
    assert.equal(partial.result.status, "complete");
    assert.equal(partial.result.output.partial, true);
    assert.equal(partial.result.output.timedOut, true);
    assert.match(partial.result.summary, /partial match/);
  } finally {
    await rm(workspace, { recursive: true, force: true });
    await rm(external, { recursive: true, force: true });
  }
});

test("shell search treats no matches as complete and reports unavailable utilities clearly", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-shell-search-status-"));
  await writeFile(join(root, "tracked.txt"), "present text\n");
  await execFileAsync("git", ["init", "--quiet", root]);
  await execFileAsync("git", ["-C", root, "add", "tracked.txt"]);
  try {
    const registry = createResearchToolRegistry([
      createShellTool({ workspaceRoot: root, authorize: allowShell }),
    ]);
    const noMatch = await registry.execute({
      id: "shell_git_grep_no_match",
      actionClass: "search",
      toolName: "shell.run",
      input: { utility: "git", args: ["-C", root, "grep", "absent text"] },
    });
    assert.equal(noMatch.result.status, "complete");
    assert.equal(noMatch.result.output.exitCode, 1);
    assert.match(noMatch.result.summary, /no matches/);

    const unavailable = await registry.execute({
      id: "shell_unavailable_utility",
      actionClass: "inspect",
      toolName: "shell.run",
      input: { utility: "app-server-missing-utility-fixture" },
    });
    assert.equal(unavailable.result.status, "error");
    assert.match(unavailable.result.summary, /not available.*PATH/);
    assert.match(unavailable.result.summary, /Do not repeat/);
    if (process.platform !== "win32") {
      assert.doesNotMatch(unavailable.result.summary, /powershell|pwsh|\bwsl\b|on Windows/iu);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("structured file read supports ranges and annotates paths outside context roots", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-file-read-"));
  const outsideRoot = await mkdtemp(join(tmpdir(), "app-server-file-outside-"));
  const file = join(root, "notes.txt");
  const outsideFile = join(outsideRoot, "outside.txt");
  await writeFile(file, "abcdef\nsecond line\n");
  await writeFile(outsideFile, "outside\n");

  try {
    const tool = createStructuredFileReadTool({
      allowedRoots: [root],
      maxBytes: 4,
    });
    const registry = createResearchToolRegistry([tool]);
    const readResult = await registry.execute({
      id: "read_1",
      actionClass: "inspect",
      toolName: "file.read",
      input: {
        path: file,
        offset: 2,
        maxBytes: 20,
      },
    });

    assert.equal(readResult.result.status, "complete");
    assert.equal(readResult.result.output.text, "cdef");
    assert.equal(readResult.result.output.bytesRead, 4);
    assert.equal(readResult.result.output.truncated, true);
    assert.equal(readResult.result.output.encoding, "utf8");
    assert.equal(readResult.result.output.containsNulByte, false);
    assert.equal(readResult.result.output.withinContextRoot, true);

    const readProjection = projectModelToolResult(readResult.result);
    const readPayload = JSON.parse(readProjection.content[0].text);
    assert.equal(readPayload.output.resolvedPath, await realpath(file));
    assert.equal(readPayload.output.text, "cdef");
    assert.equal(readPayload.output.offset, 2);
    assert.equal(readPayload.output.truncated, true);
    assert.equal("requestedPath" in readPayload.output, false);
    assert.equal("root" in readPayload.output, false);
    assert.equal("contextRoots" in readPayload.output, false);
    assert.equal("withinContextRoot" in readPayload.output, false);

    const readObserved = readResult.events.find((event) => event.kind === "tool.observed");
    assert.ok(readObserved);
    assert.deepEqual(readObserved.payload.result.contextRoots, readResult.result.output.contextRoots);
    assert.ok(readObserved.payload.fullResultCharacters > readObserved.payload.modelVisibleResultCharacters);

    const outsideResult = await registry.execute({
      id: "read_2",
      actionClass: "inspect",
      toolName: "file.read",
      input: {
        path: outsideFile,
      },
    });
    assert.equal(outsideResult.result.status, "complete");
    assert.equal(outsideResult.result.output.text, "outs");
    assert.equal(outsideResult.result.output.withinContextRoot, false);
    assert.equal(outsideResult.result.output.root, null);
    assert.match(outsideResult.result.summary, /outside workspace context hints/);
    const outsidePayload = JSON.parse(projectModelToolResult(outsideResult.result).content[0].text);
    assert.equal(outsidePayload.output.outsideContextRoots, true);

    const repeatedResult = await registry.execute(
      {
        id: "read_3",
        actionClass: "inspect",
        toolName: "file.read",
        input: {
          path: file,
        },
      },
      {
        excludedPaths: [file],
      },
    );
    assert.equal(repeatedResult.result.status, "blocked");
    assert.match(repeatedResult.result.summary, /avoid_repeated_targets/);
  } finally {
    await rm(root, {
      recursive: true,
      force: true,
    });
    await rm(outsideRoot, {
      recursive: true,
      force: true,
    });
  }
});

test("analysis tool runs deterministic metrics and diffs", async () => {
  const registry = createResearchToolRegistry([createAnalysisTool()]);
  const metrics = await registry.execute({
    id: "analysis_1",
    actionClass: "analyze",
    toolName: "analysis.transform",
    input: {
      operation: "metrics",
      text: "one two\nthree",
    },
  });
  const diff = await registry.execute({
    id: "analysis_2",
    actionClass: "analyze",
    toolName: "analysis.transform",
    input: {
      operation: "diff",
      left: "a\nb",
      right: "a\nc",
    },
  });

  assert.equal(metrics.result.status, "complete");
  assert.equal(metrics.result.output.words, 3);
  assert.equal(metrics.result.output.lines, 2);
  assert.deepEqual(diff.result.output.changes, ["-2: b", "+2: c"]);
});

test("code intelligence tools expose Tree-sitter detect, outline, query, context, references, and call candidates", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-code-tools-"));
  await mkdir(join(root, "src"));
  await symlink(root, join(root, "src", "cycle"), process.platform === "win32" ? "junction" : "dir");
  const sourcePath = join(root, "src", "parser.js");
  await writeFile(
    sourcePath,
    [
      "function parse_context_save(input) {",
      "  return input;",
      "}",
      "",
      "parse_context_save('fixture');",
      "",
    ].join("\n"),
  );

  try {
    const registry = createResearchToolRegistry(
      createCodeIntelligenceTools({
        roots: [root],
        maxFileBytes: 20_000,
      }),
    );
    for (const descriptor of registry.listDescriptors()) {
      assert.equal(descriptor.inputSchema.properties.maxBytes.maximum, 20_000);
    }
    const detect = await registry.execute({
      id: "code_detect_1",
      actionClass: "inspect",
      toolName: "code.detect",
      input: {
        path: root,
      },
    });
    const outline = await registry.execute({
      id: "code_outline_1",
      actionClass: "inspect",
      toolName: "code.outline",
      input: {
        path: "src/parser.js",
      },
    });
    const query = await registry.execute({
      id: "code_query_1",
      actionClass: "inspect",
      toolName: "code.query",
      input: {
        path: sourcePath,
        query: "(call_expression function: (identifier) @call)",
        includeText: true,
      },
    });
    const context = await registry.execute({
      id: "code_context_1",
      actionClass: "inspect",
      toolName: "code.node_context",
      input: {
        path: sourcePath,
        line: 2,
      },
    });
    const references = await registry.execute({
      id: "code_refs_1",
      actionClass: "search",
      toolName: "code.references",
      input: {
        path: "src/parser.js",
        symbol: "parse_context_save",
      },
    });
    const broadReferences = await registry.execute({
      id: "code_refs_cycle_1",
      actionClass: "search",
      toolName: "code.references",
      input: { symbol: "parse_context_save" },
    });
    const aborted = new AbortController();
    aborted.abort(new Error("fixture code scan abort"));
    const abortedScan = await registry.execute({
      id: "code_detect_aborted",
      actionClass: "inspect",
      toolName: "code.detect",
      input: { path: root },
    }, { signal: aborted.signal });
    const calls = await registry.execute({
      id: "code_calls_1",
      actionClass: "analyze",
      toolName: "code.call_candidates",
      input: {
        path: "src/parser.js",
        symbol: "parse_context_save",
      },
    });

    assert.equal(detect.result.status, "complete");
    assert.equal(detect.result.output.detections[0].language, "javascript");
    assert.equal(detect.result.output.detections[0].parseHealth.hasError, false);
    assert.equal(outline.result.status, "complete");
    assert.ok(
      outline.result.output.symbols.some(
        (symbol) =>
          symbol.kind === "definition.function" &&
          symbol.name === "parse_context_save",
      ),
    );
    assert.equal(query.result.status, "complete");
    assert.equal(query.result.output.matches[0].captures[0].text, "parse_context_save");
    assert.equal(context.result.status, "complete");
    assert.ok(
      context.result.output.ancestors.some(
        (ancestor) => ancestor.nodeType === "function_declaration",
      ),
    );
    assert.equal(references.result.status, "complete");
    assert.equal(broadReferences.result.status, "complete");
    assert.ok(broadReferences.result.output.references.length >= 1);
    assert.equal(abortedScan.result.status, "error");
    assert.match(abortedScan.result.summary, /fixture code scan abort/);
    assert.ok(
      references.result.output.references.some(
        (reference) => reference.kind === "definition.function",
      ),
    );
    assert.ok(
      references.result.output.references.some(
        (reference) => reference.kind === "reference.call",
      ),
    );
    assert.equal(calls.result.status, "complete");
    assert.deepEqual(
      calls.result.output.callCandidates.map((candidate) => candidate.kind),
      ["reference.call"],
    );
  } finally {
    await rm(root, {
      recursive: true,
      force: true,
    });
  }
});

test("experiment tool runs only allowlisted experiments", async () => {
  const registry = createResearchToolRegistry([
    createExperimentTool({
      experiments: {
        sum(input) {
          return Number(input.a) + Number(input.b);
        },
      },
    }),
  ]);
  const completed = await registry.execute({
    id: "experiment_1",
    actionClass: "experiment",
    toolName: "experiment.run",
    input: {
      name: "sum",
      input: {
        a: 2,
        b: 3,
      },
    },
  });
  const denied = await registry.execute({
    id: "experiment_2",
    actionClass: "experiment",
    toolName: "experiment.run",
    input: {
      name: "missing",
    },
  });

  assert.equal(completed.result.status, "complete");
  assert.equal(completed.result.output.output, 5);
  assert.equal(denied.result.status, "error");
  assert.match(denied.result.summary, /Unknown experiment/);
});

test("synthesis tool returns report output and artifact references", async () => {
  const result = await createResearchToolRegistry([createSynthesisTool()]).execute({
    id: "synthesis_1",
    actionClass: "synthesize",
    toolName: "synthesis.compose",
    input: {
      title: "Parser Notes",
      sections: ["Observed bounded parsing behavior."],
      artifactKind: "report",
    },
  });

  assert.equal(result.result.status, "complete");
  assert.equal(result.result.output.text, "# Parser Notes\n\nObserved bounded parsing behavior.");
  assert.equal(result.result.artifactRefs[0].kind, "report");
});

async function assertProcessExited(pid) {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      process.kill(pid, 0);
    } catch (error) {
      if (error?.code === "ESRCH") return;
      throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  assert.fail(`descendant process ${pid} remained alive after shell timeout`);
}

test("storage list tool exposes manifest artifact metadata read-only", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-storage-tool-"));
  const layout = ensureResearchStorageLayout(
    createResearchStorageLayout({ workspaceRoot }),
  );
  const artifactDir = join(layout.artifactDirectoryPath, "analysis");
  const artifactPath = join(artifactDir, "notes.txt");
  await mkdir(artifactDir, { recursive: true });
  await writeFile(artifactPath, "analysis notes\n", "utf8");
  const entry = registerResearchStorageArtifact(layout, {
    path: artifactPath,
    kind: "analysis-note",
    purpose: "Tool listing fixture.",
    sourceEventIds: ["evt_tool"],
  });

  try {
    const tool = createStorageListTool({ storageLayout: layout });
    const result = await createResearchToolRegistry([tool]).execute({
      id: "storage_1",
      actionClass: "inspect",
      toolName: "storage.list",
      input: {
        kind: "analysis-note",
      },
    });

    assert.equal(result.result.status, "complete");
    assert.equal(result.result.output.artifactCount, 1);
    assert.equal(result.result.output.artifacts[0].id, entry.id);
    assert.deepEqual(result.result.output.directories.map((directory) => directory.name), ["artifacts"]);
    assert.equal(tool.descriptor.sideEffects, "read");
    assert.deepEqual(tool.descriptor.requiredPermissions, ["storage:read"]);
  } finally {
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("default built-in family assembles configured tool surfaces", async () => {
  const root = await mkdtemp(join(tmpdir(), "app-server-family-"));
  try {
    const tools = createDefaultBuiltInToolFamily({
      repositorySearch: {
        root,
      },
      fileRead: {
        allowedRoots: [root],
      },
      code: {
        roots: [root],
      },
      experiments: {
        experiments: {
          noop() {
            return "ok";
          },
        },
      },
    });
    const names = tools.map((tool) => tool.descriptor.name).sort();
    const transportNames = createResearchToolRegistry(tools)
      .toPiTools()
      .map((tool) => tool.name)
      .sort();

    assert.deepEqual(names, [
      "analysis.transform",
      "code.call_candidates",
      "code.detect",
      "code.node_context",
      "code.outline",
      "code.query",
      "code.references",
      "experiment.run",
      "file.read",
      "prior_art.search",
      "repository.history",
      "repository.search",
      "synthesis.compose",
    ]);
    assert.deepEqual(transportNames, [
      "analysis_transform",
      "code_call_candidates",
      "code_detect",
      "code_node_context",
      "code_outline",
      "code_query",
      "code_references",
      "experiment_run",
      "file_read",
      "prior_art_search",
      "repository_history",
      "repository_search",
      "synthesis_compose",
    ]);
  } finally {
    await rm(root, {
      recursive: true,
      force: true,
    });
  }
});
