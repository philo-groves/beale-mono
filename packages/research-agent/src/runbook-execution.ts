import { createHash, randomUUID } from "node:crypto";
import { chmodSync, closeSync, copyFileSync, existsSync, mkdirSync, openSync, readSync, realpathSync, statSync } from "node:fs";
import { basename, isAbsolute, relative, resolve } from "node:path";
import type { ResearchExecutableTool, ResearchToolExecutionResult } from "./tool-registry.js";
import { registerResearchStorageArtifact, resolveResearchStorageArtifact } from "./storage.js";
import { MAX_MCP_CALL_TIMEOUT_MS } from "./mcp-tools.js";
import type { ResearchArtifactRef, ResearchStorageLayout, ResearchToolAction } from "./types.js";
import {
  RUNBOOK_PROOF_TARGETS,
  RunbookStore,
  type RunbookExecutionPlanCell,
  type RunbookCellExecutor,
  type RunbookProofTarget,
} from "./runbooks.js";

export interface RunbookExecutionRequest {
  runbookId: string;
  cellId?: string;
  startCellId?: string;
  endCellId?: string;
  signal?: AbortSignal;
  proofTarget: RunbookProofTarget;
  deviceOs?: string;
  sourceRevision?: string;
  environmentFingerprint?: string;
  /** Host execution identity; not part of the model input schema. */
  actorId?: string;
}

export interface RunbookExecutionUpdate {
  type: "runbook_execution";
  runbookId: string;
  runId: string;
  cellId: string | null;
  status: "running" | "succeeded" | "failed" | "blocked" | "skipped";
  durationMs?: number;
  error?: string;
  proofTarget: RunbookProofTarget;
  deviceOs?: string;
}

export interface RunbookExecutorOptions {
  store: RunbookStore;
  shellTool: ResearchExecutableTool;
  tartVm?: {
    workspaceRoot: string;
    storageLayout: ResearchStorageLayout;
    inspectTool: ResearchExecutableTool;
    copyTool: ResearchExecutableTool;
    execTool: ResearchExecutableTool;
  };
  signal?: AbortSignal;
  onUpdate?(update: RunbookExecutionUpdate): void | Promise<void>;
}

export interface RunbookExecutionResult {
  runbookId: string;
  title: string;
  runId: string;
  status: "succeeded" | "failed" | "blocked";
  startedAt: string;
  completedAt: string;
  durationMs: number;
  error?: string;
  proofTarget: RunbookProofTarget;
  deviceOs?: string;
}

export function createRunbookExecutor(options: RunbookExecutorOptions): (
  request: RunbookExecutionRequest,
) => Promise<RunbookExecutionResult> {
  const activeRunbooks = new Set<string>();
  return async (request) => {
    const runbookId = requiredText(request.runbookId, "runbookId");
    const proofTarget = parseProofTarget(request.proofTarget);
    const deviceOs = proofTarget === "device" ? requiredText(request.deviceOs, "deviceOs") : undefined;
    if (activeRunbooks.has(runbookId)) throw new Error(`Runbook is already executing: ${runbookId}`);
    const runbook = options.store.get(runbookId, { limit: 1 });
    if (!runbook) throw new Error(`Runbook not found in this workspace: ${runbookId}`);
    const cells = options.store.executionPlan(runbookId, {
      ...(request.cellId ? { cellId: request.cellId } : {}),
      ...(request.startCellId ? { startCellId: request.startCellId } : {}),
      ...(request.endCellId ? { endCellId: request.endCellId } : {}),
    });
    const runId = `runbook_run_${randomUUID()}`;
    const startedAt = new Date().toISOString();
    const startedMs = Date.now();
    activeRunbooks.add(runbookId);
    let finalStatus: "succeeded" | "failed" | "blocked" = "succeeded";
    let finalError: string | undefined;
    let completedCount = 0;
    let completedAt = startedAt;
    let durationMs = 0;
    try {
      options.store.beginExecution(runbookId, runId, cells.map((cell) => cell.id), proofTarget, deviceOs, {
        expectedContentRevision: runbook.contentRevision,
        ...(request.sourceRevision ? { sourceRevision: request.sourceRevision } : {}),
        ...(request.environmentFingerprint ? { environmentFingerprint: request.environmentFingerprint } : {}),
        ...(request.actorId ? { actorId: request.actorId } : {}),
      });
    } catch (error) {
      activeRunbooks.delete(runbookId);
      throw error;
    }
    try {
      await options.onUpdate?.({ type: "runbook_execution", runbookId, runId, cellId: null, status: "running", proofTarget, ...(deviceOs ? { deviceOs } : {}) });
      for (const cell of cells) {
        const signal = request.signal ?? options.signal;
        throwIfAborted(signal);
        const cellStartedAt = new Date().toISOString();
        const cellStartedMs = Date.now();
        options.store.beginCellExecution(runbookId, runId, cell.id, proofTarget, deviceOs);
        await options.onUpdate?.({ type: "runbook_execution", runbookId, runId, cellId: cell.id, status: "running", proofTarget, ...(deviceOs ? { deviceOs } : {}) });
        let result: ResearchToolExecutionResult;
        try {
          result = await executeCell(options, runbookId, runId, cell, proofTarget, signal);
        } catch (error) {
          const message = errorMessage(error);
          const completedAt = new Date().toISOString();
          const durationMs = Math.max(0, Date.now() - cellStartedMs);
          options.store.completeCellExecution({
            id: runbookId,
            runId,
            cellId: cell.id,
            status: "failed",
            startedAt: cellStartedAt,
            completedAt,
            durationMs,
            error: message,
            proofTarget,
            ...(deviceOs ? { deviceOs } : {}),
          });
          completedCount += 1;
          await options.onUpdate?.({
            type: "runbook_execution",
            runbookId,
            runId,
            cellId: cell.id,
            status: "failed",
            durationMs,
            error: message,
            proofTarget,
            ...(deviceOs ? { deviceOs } : {}),
          });
          finalStatus = "failed";
          finalError = message;
          break;
        }
        const output = shellOutput(result.output);
        const status = result.status === "complete" ? "succeeded" : result.status === "error" ? "failed" : "blocked";
        const completedAt = new Date().toISOString();
        const durationMs = Math.max(0, Date.now() - cellStartedMs);
        const evidence = executionEvidence(result.output);
        options.store.completeCellExecution({
          id: runbookId,
          runId,
          cellId: cell.id,
          status,
          startedAt: cellStartedAt,
          completedAt,
          durationMs,
          ...(output.stdout ? { stdout: output.stdout } : {}),
          ...(output.stderr ? { stderr: output.stderr } : {}),
          ...(output.exitCode !== undefined ? { exitCode: output.exitCode } : {}),
          ...(result.error?.message ? { error: result.error.message } : {}),
          proofTarget,
          ...(deviceOs ? { deviceOs } : {}),
          ...(evidence ? { evidence } : {}),
        });
        completedCount += 1;
        await options.onUpdate?.({
          type: "runbook_execution",
          runbookId,
          runId,
          cellId: cell.id,
          status,
          durationMs,
          ...(result.error?.message ? { error: result.error.message } : {}),
          proofTarget,
          ...(deviceOs ? { deviceOs } : {}),
        });
        if (status !== "succeeded") {
          finalStatus = status;
          finalError = result.error?.message ?? result.summary;
          break;
        }
      }
    } catch (error) {
      finalStatus = "failed";
      finalError = errorMessage(error);
    } finally {
      const skipped = cells.slice(completedCount).map((cell) => cell.id);
      if (skipped.length > 0) {
        options.store.skipCellExecutions(runbookId, runId, skipped, finalError ?? "Skipped after an earlier cell did not succeed.", proofTarget, deviceOs);
        for (const cellId of skipped) {
          await options.onUpdate?.({
            type: "runbook_execution",
            runbookId,
            runId,
            cellId,
            status: "skipped",
            durationMs: 0,
            error: finalError ?? "Skipped after an earlier cell did not succeed.",
            proofTarget,
            ...(deviceOs ? { deviceOs } : {}),
          });
        }
      }
      completedAt = new Date().toISOString();
      durationMs = Math.max(0, Date.now() - startedMs);
      options.store.completeExecution({
        id: runbookId,
        runId,
        status: finalStatus,
        startedAt,
        completedAt,
        durationMs,
        ...(finalError ? { error: finalError } : {}),
        proofTarget,
        ...(deviceOs ? { deviceOs } : {}),
      });
      activeRunbooks.delete(runbookId);
      await options.onUpdate?.({
        type: "runbook_execution",
        runbookId,
        runId,
        cellId: null,
        status: finalStatus,
        durationMs,
        ...(finalError ? { error: finalError } : {}),
        proofTarget,
        ...(deviceOs ? { deviceOs } : {}),
      });
    }
    return {
      runbookId,
      title: runbook.title,
      runId,
      status: finalStatus,
      startedAt,
      completedAt,
      durationMs,
      ...(finalError ? { error: finalError } : {}),
      proofTarget,
      ...(deviceOs ? { deviceOs } : {}),
    };
  };
}

export function createRunbookExecutionTool(
  executeRunbook: (request: RunbookExecutionRequest) => Promise<RunbookExecutionResult>,
): ResearchExecutableTool {
  const parameters = {
    type: "object",
    required: ["id", "proofTarget"],
    properties: {
      id: { type: "string", description: "Runbook ID to execute." },
      cellId: { type: "string", description: "Optional exact code cell ID from runbook.get. Cannot be combined with a range." },
      startCellId: { type: "string", description: "Optional inclusive first code cell ID from runbook.get. Omit to start at the first code cell." },
      endCellId: { type: "string", description: "Optional inclusive last code cell ID from runbook.get. Omit to continue through the final code cell." },
      proofTarget: { type: "string", enum: [...RUNBOOK_PROOF_TARGETS], description: "Where this proof executes: localhost, device, vm, web, or other." },
      deviceOs: { type: "string", description: "Required when proofTarget is device, for example iOS 27.0 or Android 17." },
      sourceRevision: { type: "string", description: "Exact inspected source/build identity. Required for finding reproduction evidence; must match the claim's sourceRevision." },
      environmentFingerprint: { type: "string", description: "Exact execution environment identity. Required for finding reproduction evidence; must match the claim's environmentFingerprint." },
    },
  };
  return {
    descriptor: {
      name: "runbook.run",
      transportName: "runbook_run",
      description: "Execute active code cells from one cell, an inclusive ordered range, or a complete runbook. Host cells use the app-server shell safety boundary. Tart VM cells automatically materialize a referenced host-built executable, stream it to the named guest, inspect immediately before execution, invoke it as the Guest Agent service identity or through passwordless sudo according to runAs, capture output and guest evidence, and clean up; cells whose feature tags are all disabled are skipped. This is the required execution path for proof-of-concepts, vulnerability reproductions, exploit-path tests, verifiers, claim-confirming experiments, and evidence benchmarks. Returns the durable runId required for reproduction-grade runbook_execution finding evidence. Use startCellId after repairing a late failure so already-successful cells are not repeated.",
      actionClasses: ["experiment"],
      sideEffects: "process",
      requiredPermissions: ["process:spawn"],
      inputSchema: parameters,
      metadata: { family: "runbook", format: "jupyter-nbformat-4", proofBoundary: true },
    },
    parameters,
    async execute(action, context) {
      const startedAt = new Date().toISOString();
      try {
        const result = await executeRunbook({
          runbookId: requiredText(action.input.id, "id"),
          ...(typeof action.input.cellId === "string" && action.input.cellId.trim()
            ? { cellId: action.input.cellId.trim() }
            : {}),
          ...(typeof action.input.startCellId === "string" && action.input.startCellId.trim()
            ? { startCellId: action.input.startCellId.trim() }
            : {}),
          ...(typeof action.input.endCellId === "string" && action.input.endCellId.trim()
            ? { endCellId: action.input.endCellId.trim() }
            : {}),
          ...(context?.signal ? { signal: context.signal } : {}),
          ...(context?.agentId ? { actorId: context.agentId } : {}),
          ...(action.input.sourceRevision !== undefined ? { sourceRevision: requiredText(action.input.sourceRevision, "sourceRevision") } : {}),
          ...(action.input.environmentFingerprint !== undefined ? { environmentFingerprint: requiredText(action.input.environmentFingerprint, "environmentFingerprint") } : {}),
          proofTarget: parseProofTarget(action.input.proofTarget),
          ...(typeof action.input.deviceOs === "string" && action.input.deviceOs.trim()
            ? { deviceOs: action.input.deviceOs.trim() }
            : {}),
        });
        return {
          action,
          status: "complete",
          startedAt,
          completedAt: new Date().toISOString(),
          summary: `Runbook execution ${result.status}: ${result.runId}.`,
          output: result,
          followUpActions: [],
        };
      } catch (error) {
        return {
          action,
          status: "error",
          startedAt,
          completedAt: new Date().toISOString(),
          summary: "Runbook execution failed.",
          error: { message: errorMessage(error) },
          followUpActions: ["Inspect the recorded cell result and repair the runbook before retrying."],
        };
      }
    },
  };
}

async function executeCell(
  options: RunbookExecutorOptions,
  runbookId: string,
  runId: string,
  cell: RunbookExecutionPlanCell,
  proofTarget: RunbookProofTarget,
  signal?: AbortSignal,
): Promise<ResearchToolExecutionResult> {
  if (cell.executor.kind === "tart-vm") {
    if (proofTarget !== "vm") throw new Error(`Runbook code cell ${cell.id} uses tart-vm and requires proofTarget vm.`);
    if (!options.tartVm) throw new Error("Tart VM runbook execution is unavailable because the managed device tools are not active.");
    return executeTartVmCell(options.tartVm, runbookId, runId, cell, cell.executor, signal);
  }
  const action: ResearchToolAction = {
    id: `runbook_cell_${randomUUID()}`,
    actionClass: "experiment",
    toolName: "shell.run",
    input: cellInvocation(cell),
  };
  return options.shellTool.execute(action, {
    ...(signal ? { signal } : {}),
    runbookContext: { runbookId, runId, cellId: cell.id },
  });
}

async function executeTartVmCell(
  tools: NonNullable<RunbookExecutorOptions["tartVm"]>,
  runbookId: string,
  runId: string,
  cell: RunbookExecutionPlanCell,
  executor: Extract<RunbookCellExecutor, { kind: "tart-vm" }>,
  signal?: AbortSignal,
): Promise<ResearchToolExecutionResult> {
  const startedAt = new Date().toISOString();
  const materialized = materializeGuestExecutable(tools, executor);
  const guestPath = `/tmp/.beale-runbook-${materialized.contentHash.replace(/^sha256:/u, "").slice(0, 24)}-${safeGuestName(basename(materialized.path))}`;
  const context = { ...(signal ? { signal } : {}), runbookContext: { runbookId, runId, cellId: cell.id } };
  let execution: ResearchToolExecutionResult | undefined;
  let staged = false;
  let cleaned = false;
  let retained = false;
  let cleanupError: string | undefined;
  try {
    const copied = await executeInternalTool(tools.copyTool, {
      vmName: executor.vmName,
      localPath: materialized.path,
      guestPath,
      overwrite: true,
      preserveMode: true,
      transport: executor.transport,
      timeoutSeconds: 900,
      bealeTimeoutMs: 15 * 60_000,
    }, context);
    requireComplete(copied, "stage the guest executable");
    staged = true;
    const inspected = await executeInternalTool(tools.inspectTool, { vmName: executor.vmName, transport: executor.transport }, context);
    requireComplete(inspected, "inspect the Tart VM before execution");
    execution = await executeInternalTool(tools.execTool, {
      vmName: executor.vmName,
      transport: executor.transport,
      argv: executor.runAs === "root"
        ? ["/usr/bin/sudo", "--", guestPath, ...executor.argv]
        : [guestPath, ...executor.argv],
      timeoutSeconds: executor.timeoutSeconds,
      bealeTimeoutMs: Math.min(
        MAX_MCP_CALL_TIMEOUT_MS,
        Math.max(120_000, (executor.timeoutSeconds + 30) * 1_000),
      ),
    }, context);
    if (execution.status !== "complete") retained = executor.retainOnFailure && staged;
  } catch (error) {
    retained = executor.retainOnFailure && staged;
    execution = failedCellResult(cell, startedAt, error);
  } finally {
    if (staged && !retained) {
      try {
        const cleanup = await executeInternalTool(tools.execTool, {
          vmName: executor.vmName,
          transport: executor.transport,
          argv: ["/bin/rm", "-f", guestPath],
          timeoutSeconds: 30,
          bealeTimeoutMs: 120_000,
        }, context);
        if (cleanup.status === "complete") cleaned = true;
        else cleanupError = mcpDiagnostic(cleanup) ?? cleanup.error?.message ?? cleanup.summary;
      } catch (error) {
        cleanupError = errorMessage(error);
      }
      if (cleanupError) retained = true;
    }
  }
  const command = execution ?? failedCellResult(cell, startedAt, new Error("Tart VM execution did not return a result."));
  const parsed = parseMcpTextOutput(command);
  const evidence = {
    executor: "tart-vm",
    vmName: executor.vmName,
    runAs: executor.runAs,
    artifactId: materialized.id,
    contentHash: materialized.contentHash,
    guestPath,
    staged,
    cleaned,
    retained,
    ...(cleanupError ? { cleanupError: cleanupError.slice(0, 500) } : {}),
    ...(typeof parsed.transport === "string" ? { transport: parsed.transport } : {}),
  };
  const diagnostic = command.status === "complete" ? undefined : mcpDiagnostic(command);
  return {
    action: command.action,
    status: command.status,
    startedAt,
    completedAt: new Date().toISOString(),
    summary: command.status === "complete" ? "Tart VM runbook cell completed." : "Tart VM runbook cell failed.",
    output: {
      ...(typeof parsed.stdout === "string" ? { stdout: parsed.stdout } : {}),
      ...(typeof parsed.stderr === "string" ? { stderr: parsed.stderr } : diagnostic ? { stderr: diagnostic } : {}),
      ...(typeof parsed.exitCode === "number" || parsed.exitCode === null ? { exitCode: parsed.exitCode as number | null } : {}),
      guestExecution: evidence,
    },
    artifactRefs: [artifactRef(materialized)],
    followUpActions: command.followUpActions,
    ...(command.status === "complete" ? {} : { error: { message: diagnostic ?? command.error?.message ?? "Tart VM execution failed." } }),
  };
}

function materializeGuestExecutable(
  tools: NonNullable<RunbookExecutorOptions["tartVm"]>,
  executor: Extract<RunbookCellExecutor, { kind: "tart-vm" }>,
) {
  if (executor.artifactId) {
    const artifact = resolveResearchStorageArtifact(tools.storageLayout, executor.artifactId);
    if (!artifact) throw new Error(`Guest executable artifact not found: ${executor.artifactId}`);
    requireExecutableFile(artifact.path);
    return artifact;
  }
  if (isAbsolute(executor.workspacePath!)) throw new Error("Guest executable workspacePath must be relative to the workspace root.");
  const workspaceRoot = realpathSync(tools.workspaceRoot);
  const candidate = realpathSync(resolve(workspaceRoot, executor.workspacePath!));
  const relation = relative(workspaceRoot, candidate);
  if (!relation || relation === ".." || relation.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`)) {
    throw new Error("Guest executable workspacePath escapes the workspace root.");
  }
  const file = requireExecutableFile(candidate);
  const contentHash = hashFile(candidate);
  const destinationDirectory = resolve(tools.storageLayout.artifactDirectoryPath, "runbook-packages");
  mkdirSync(destinationDirectory, { recursive: true, mode: 0o700 });
  const destination = resolve(destinationDirectory, `${contentHash.slice(0, 32)}-${safeGuestName(basename(candidate))}`);
  if (!existsSync(destination) || hashFile(destination) !== contentHash) {
    copyFileSync(candidate, destination);
  }
  if (hashFile(destination) !== contentHash) throw new Error("Guest executable changed while it was being materialized.");
  chmodSync(destination, file.mode & 0o777);
  return registerResearchStorageArtifact(tools.storageLayout, {
    path: destination,
    kind: "runbook-guest-executable",
    purpose: "Host-built executable materialized for a runbook guest execution.",
  });
}

function hashFile(path: string): string {
  const hash = createHash("sha256");
  const handle = openSync(path, "r");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  try {
    for (;;) {
      const bytesRead = readSync(handle, buffer, 0, buffer.length, null);
      if (bytesRead === 0) break;
      hash.update(buffer.subarray(0, bytesRead));
    }
  } finally {
    closeSync(handle);
  }
  return hash.digest("hex");
}

function requireExecutableFile(path: string) {
  if (!existsSync(path)) throw new Error("Guest executable does not exist.");
  const file = statSync(path);
  if (!file.isFile()) throw new Error("Guest executable must be a regular file.");
  if ((file.mode & 0o111) === 0) throw new Error("Guest executable must have an executable permission bit.");
  return file;
}

async function executeInternalTool(tool: ResearchExecutableTool, input: Record<string, unknown>, context: Parameters<ResearchExecutableTool["execute"]>[1]) {
  return tool.execute({
    id: `runbook_vm_${randomUUID()}`,
    toolName: tool.descriptor.name,
    actionClass: tool.descriptor.actionClasses[0] ?? "experiment",
    input,
  }, context);
}

function requireComplete(result: ResearchToolExecutionResult, operation: string): void {
  if (result.status !== "complete") throw new Error(`${operation} failed: ${mcpDiagnostic(result) ?? result.error?.message ?? result.summary}`);
}

function failedCellResult(cell: RunbookExecutionPlanCell, startedAt: string, error: unknown): ResearchToolExecutionResult {
  const message = errorMessage(error);
  return {
    action: { id: `runbook_vm_${randomUUID()}`, toolName: "runbook.tart-vm", actionClass: "experiment", input: { cellId: cell.id } },
    status: "error",
    startedAt,
    completedAt: new Date().toISOString(),
    summary: "Tart VM runbook cell failed.",
    error: { message },
    followUpActions: [],
  };
}

function parseMcpTextOutput(result: ResearchToolExecutionResult): Record<string, unknown> {
  const text = result.modelContent?.find((item) => item.type === "text")?.text;
  if (!text) return {};
  try {
    const parsed = JSON.parse(text) as unknown;
    return isRecord(parsed) ? parsed : {};
  } catch {
    return {};
  }
}

function mcpDiagnostic(result: ResearchToolExecutionResult): string | undefined {
  return result.modelContent?.find((item) => item.type === "text")?.text?.slice(0, 2_000);
}

function executionEvidence(value: unknown): Record<string, string | number | boolean | null> | undefined {
  if (!isRecord(value) || !isRecord(value.guestExecution)) return undefined;
  return Object.fromEntries(Object.entries(value.guestExecution).filter((entry): entry is [string, string | number | boolean | null] => {
    const candidate = entry[1];
    return typeof candidate === "string" || typeof candidate === "number" || typeof candidate === "boolean" || candidate === null;
  }));
}

function artifactRef(artifact: ReturnType<typeof registerResearchStorageArtifact>): ResearchArtifactRef {
  return { id: artifact.id, kind: artifact.kind, uri: artifact.uri, summary: artifact.purpose, contentHash: artifact.contentHash };
}

function safeGuestName(value: string): string {
  return value.replace(/[^A-Za-z0-9._-]/gu, "_").slice(0, 80) || "executable";
}

function cellInvocation(cell: RunbookExecutionPlanCell): Record<string, unknown> {
  const language = cell.language?.trim().toLowerCase();
  if (!language) throw new Error(`Runbook code cell ${cell.id} requires an explicit language.`);
  const timeoutMs = cell.executor.kind === "host" ? cell.executor.timeoutSeconds * 1_000 : undefined;
  if (["shell", "sh", "posix-shell"].includes(language)) return { command: cell.source, timeoutMs };
  if (language === "bash") return { utility: "bash", args: ["-lc", cell.source], timeoutMs };
  if (language === "zsh") return { utility: "zsh", args: ["-lc", cell.source], timeoutMs };
  if (["python", "python3", "py"].includes(language)) return { utility: "python3", args: ["-c", cell.source], timeoutMs };
  if (["javascript", "js", "node"].includes(language)) return { utility: "node", args: ["-e", cell.source], timeoutMs };
  if (language === "ruby") return { utility: "ruby", args: ["-e", cell.source], timeoutMs };
  if (language === "perl") return { utility: "perl", args: ["-e", cell.source], timeoutMs };
  if (["powershell", "pwsh"].includes(language)) return { utility: "pwsh", args: ["-NoProfile", "-Command", cell.source], timeoutMs };
  throw new Error(`Runbook code cell ${cell.id} uses unsupported language ${cell.language}.`);
}

function shellOutput(value: unknown): { stdout?: string; stderr?: string; exitCode?: number | null } {
  if (!isRecord(value)) return {};
  return {
    ...(typeof value.stdout === "string" ? { stdout: value.stdout } : {}),
    ...(typeof value.stderr === "string" ? { stderr: value.stderr } : {}),
    ...(typeof value.exitCode === "number" || value.exitCode === null ? { exitCode: value.exitCode as number | null } : {}),
  };
}

function requiredText(value: unknown, field: string): string {
  if (typeof value !== "string" || !value.trim()) throw new Error(`${field} must be a non-empty string.`);
  return value.trim();
}

function parseProofTarget(value: unknown): RunbookProofTarget {
  if (typeof value === "string" && RUNBOOK_PROOF_TARGETS.includes(value as RunbookProofTarget)) {
    return value as RunbookProofTarget;
  }
  throw new Error(`proofTarget must be one of: ${RUNBOOK_PROOF_TARGETS.join(", ")}.`);
}

function throwIfAborted(signal: AbortSignal | undefined): void {
  if (signal?.aborted) throw new Error("Runbook execution was aborted.");
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
