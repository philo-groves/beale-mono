import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";

import {
  createResearchStorageLayout,
  createResearchToolRegistry,
  createRunbookExecutor,
  createRunbookExecutionTool,
  createRunbookTools,
  ensureResearchStorageLayout,
  getDefaultMemoryDatabasePath,
  listResearchStorageArtifacts,
  RunbookStore,
} from "../packages/research-agent/dist/index.js";

test("runbook tools expose bounded artifact operations", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-runbook-tools-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const store = new RunbookStore(
    getDefaultMemoryDatabasePath(workspaceRoot),
    layout,
    { sessionId: "run_tools", workspaceId: `workspace_tools_${randomUUID()}`, workspaceName: "Tools" },
  );
  const registry = createResearchToolRegistry(createRunbookTools(store));
  try {
    const descriptors = registry.listDescriptors();
    assert.deepEqual(descriptors.map((tool) => tool.name), ["runbook.list", "runbook.get", "runbook.create", "runbook.append", "runbook.configure"]);
    assert.equal("statuses" in descriptors.find((tool) => tool.name === "runbook.list").inputSchema.properties, false);
    assert.equal("status" in descriptors.find((tool) => tool.name === "runbook.create").inputSchema.properties, false);
    assert.equal("status" in descriptors.find((tool) => tool.name === "runbook.append").inputSchema.properties, false);
    assert.match(descriptors.find((tool) => tool.name === "runbook.create").description, /setup, runtime, and cleanup in one cohesive runbook/);
    assert.doesNotMatch(descriptors.find((tool) => tool.name === "runbook.create").description, /4–12|medium-sized/);
    assert.match(descriptors.find((tool) => tool.name === "runbook.append").description, /only for a genuinely unrelated objective/);
    assert.equal(descriptors.find((tool) => tool.name === "runbook.create").inputSchema.properties.cells.maxItems, 100);
    const executorSchema = descriptors.find((tool) => tool.name === "runbook.create").inputSchema.properties.cells.items.properties.executor;
    assert.deepEqual(executorSchema.properties.runAs.enum, ["guest", "root"]);
    assert.equal(executorSchema.properties.runAs.default, "guest");
    assert.equal(executorSchema.properties.timeoutSeconds.default, 300);
    assert.equal(executorSchema.properties.timeoutSeconds.maximum, 1800);
    assert.ok(descriptors.find((tool) => tool.name === "runbook.configure").inputSchema.properties.cellExecutors);
    const created = await registry.execute({
      id: "create_runbook",
      actionClass: "synthesize",
      toolName: "runbook.create",
      input: { title: "Crash triage", purpose: "Repeatable crash collection and classification." },
    });
    assert.equal(created.result.status, "complete");
    assert.equal(created.result.artifactRefs[0].kind, "runbook");
    assert.deepEqual(created.result.output.enabledFeatures, ["setup", "runtime", "cleanup"]);
    const appended = await registry.execute({
      id: "append_large_workflow",
      actionClass: "synthesize",
      toolName: "runbook.append",
      input: {
        id: created.result.output.id,
        expectedRevision: created.result.output.revision,
        cells: Array.from({ length: 21 }, (_, index) => ({
          kind: "markdown",
          source: `Workflow step ${index + 1}`,
          features: [index === 0 ? "setup" : index === 20 ? "cleanup" : "runtime"],
        })),
      },
    });
    assert.equal(appended.result.status, "complete");
    assert.equal(appended.result.output.cellCount, 22);

    const firstPage = store.get(created.result.output.id, { limit: 5 });
    assert.equal(firstPage.cells.length, 5);
    assert.equal(firstPage.nextOffset, 5);
    assert.equal(firstPage.previousOffset, null);
    const secondPage = store.get(created.result.output.id, { offset: firstPage.nextOffset, limit: 5 });
    assert.equal(secondPage.offset, 5);
    assert.equal(secondPage.previousOffset, 0);
    const range = store.get(created.result.output.id, {
      startCellId: firstPage.cells[1].id,
      endCellId: firstPage.cells[3].id,
      limit: 10,
    });
    assert.deepEqual(range.cells.map((cell) => cell.id), firstPage.cells.slice(1, 4).map((cell) => cell.id));
    const truncatedRange = store.get(created.result.output.id, {
      startCellId: firstPage.cells[1].id,
      endCellId: firstPage.cells[3].id,
      limit: 2,
    });
    assert.deepEqual(truncatedRange.cells.map((cell) => cell.id), firstPage.cells.slice(1, 3).map((cell) => cell.id));
    assert.equal(truncatedRange.nextOffset, null);
    assert.equal(truncatedRange.previousOffset, null);
    assert.equal(truncatedRange.nextCellId, firstPage.cells[3].id);
    const rangeTail = store.get(created.result.output.id, {
      startCellId: truncatedRange.nextCellId,
      endCellId: firstPage.cells[3].id,
      limit: 2,
    });
    assert.deepEqual(rangeTail.cells.map((cell) => cell.id), [firstPage.cells[3].id]);
    assert.equal(rangeTail.nextCellId, null);

    const listed = await registry.execute({ id: "list_runbooks", actionClass: "recall", toolName: "runbook.list", input: {} });
    assert.equal(listed.result.output.total, 1);
    assert.equal(listed.result.output.runbooks[0].id, created.result.output.id);
    const unchanged = await registry.execute({
      id: "list_runbooks_unchanged",
      actionClass: "recall",
      toolName: "runbook.list",
      input: { afterRevision: listed.result.output.revision },
    });
    assert.equal(unchanged.result.output.unchanged, true);
    assert.deepEqual(unchanged.result.output.runbooks, []);
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("runbook execution stages host-built executables into Tart VMs and records guest evidence", {
  skip: process.platform === "win32" ? "Tart executable staging requires POSIX permission bits." : false,
}, async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-runbook-tart-vm-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const store = new RunbookStore(
    getDefaultMemoryDatabasePath(workspaceRoot),
    layout,
    { sessionId: "session_tart_vm", workspaceId: "workspace_tart_vm", workspaceName: "Tart VM" },
  );
  const executablePath = join(workspaceRoot, "guest-probe");
  await writeFile(executablePath, "#!/bin/sh\nprintf 'guest proof passed\\n'\n", "utf8");
  await chmod(executablePath, 0o755);
  const calls = [];
  const tool = (name, execute) => ({
    descriptor: { name, description: "fixture", actionClasses: ["experiment"], sideEffects: "process", requiredPermissions: [] },
    execute,
  });
  const complete = (action, modelOutput) => ({
    action,
    status: "complete",
    startedAt: new Date().toISOString(),
    completedAt: new Date().toISOString(),
    summary: "complete",
    modelContent: modelOutput === undefined ? [] : [{ type: "text", text: JSON.stringify(modelOutput) }],
    followUpActions: [],
  });
  const copyTool = tool("mcp.apple-security-devices.devices.copy_to_tart_vm", async (action) => {
    calls.push({ operation: "copy", input: action.input });
    return complete(action);
  });
  const inspectTool = tool("mcp.apple-security-devices.devices.inspect_tart_vm", async (action) => {
    calls.push({ operation: "inspect", input: action.input });
    return complete(action);
  });
  const execTool = tool("mcp.apple-security-devices.devices.exec_tart_vm", async (action) => {
    const cleanup = action.input.argv[0] === "/bin/rm";
    calls.push({ operation: cleanup ? "cleanup" : "exec", input: action.input });
    return complete(action, cleanup ? {} : {
      stdout: "guest proof passed\n",
      stderr: "",
      exitCode: 0,
      transport: "tart-exec",
    });
  });
  const shellTool = tool("shell.run", async () => {
    throw new Error("Tart VM cells must not execute through the host shell.");
  });
  try {
    const created = store.create({
      title: "Guest proof sequence",
      purpose: "Build on the host and execute the packaged verifier in an isolated guest.",
      cells: [{
        kind: "code",
        language: "sh",
        source: "Execute the host-built guest probe.",
        features: ["runtime"],
      }],
    });
    const codeCellId = store.get(created.runbook.id).cells.find((cell) => cell.kind === "code").id;
    store.configure({
      id: created.runbook.id,
      expectedRevision: created.runbook.revision,
      enabledFeatures: ["setup", "runtime", "cleanup"],
      cellExecutors: [{
        cellId: codeCellId,
        executor: {
          kind: "tart-vm",
          vmName: "example-vm",
          workspacePath: "guest-probe",
          runAs: "root",
          transport: "ssh",
          argv: ["--verify"],
          timeoutSeconds: 1800,
          retainOnFailure: false,
        },
      }],
    });
    const execute = createRunbookExecutor({
      store,
      shellTool,
      tartVm: { workspaceRoot, storageLayout: layout, inspectTool, copyTool, execTool },
    });

    const wrongTarget = await execute({ runbookId: created.runbook.id, proofTarget: "localhost" });
    assert.equal(wrongTarget.status, "failed");
    assert.match(wrongTarget.error, /requires proofTarget vm/);
    const result = await execute({ runbookId: created.runbook.id, proofTarget: "vm" });

    assert.equal(result.status, "succeeded", result.error);
    assert.deepEqual(calls.map((call) => call.operation), ["copy", "inspect", "exec", "cleanup"]);
    assert.equal(calls[0].input.vmName, "example-vm");
    assert.match(calls[0].input.localPath, /runbook-packages/);
    assert.equal(calls[0].input.preserveMode, true);
    assert.equal(calls[0].input.transport, "ssh");
    assert.equal(calls[1].input.transport, "ssh");
    assert.deepEqual(calls[2].input.argv.slice(0, 3), ["/usr/bin/sudo", "--", calls[2].input.argv[2]]);
    assert.match(calls[2].input.argv[2], /^\/tmp\/\.beale-runbook-/);
    assert.deepEqual(calls[2].input.argv.slice(3), ["--verify"]);
    assert.equal(calls[2].input.timeoutSeconds, 1800);
    assert.equal(calls[2].input.transport, "ssh");
    assert.equal(calls[2].input.bealeTimeoutMs, 1_800_000);
    assert.deepEqual(calls[3].input.argv.slice(0, 2), ["/bin/rm", "-f"]);
    assert.equal(calls[3].input.argv[2], calls[2].input.argv[2]);
    assert.equal(listResearchStorageArtifacts(layout, { kind: "runbook-guest-executable" }).length, 1);

    const artifact = listResearchStorageArtifacts(layout, { kind: "runbook" })
      .find((candidate) => candidate.id === created.runbook.artifactId);
    const notebook = JSON.parse(await readFile(artifact.path, "utf8"));
    const codeCell = notebook.cells[1];
    assert.equal(codeCell.metadata.beale.executor.kind, "tart-vm");
    assert.equal(codeCell.metadata.beale.executor.runAs, "root");
    assert.equal(codeCell.metadata.beale.latestRun.evidence.runAs, "root");
    assert.equal(codeCell.metadata.beale.latestRun.evidence.vmName, "example-vm");
    assert.equal(codeCell.metadata.beale.latestRun.evidence.transport, "tart-exec");
    assert.equal(codeCell.metadata.beale.latestRun.evidence.cleaned, true);
    assert.equal(codeCell.outputs[0].text.join(""), "guest proof passed\n");
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("runbook tools advertise PowerShell cells only on Windows", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-runbook-platform-guidance-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const store = new RunbookStore(
    getDefaultMemoryDatabasePath(workspaceRoot),
    layout,
    { sessionId: "run_platform", workspaceId: "workspace_platform", workspaceName: "Platform" },
  );
  try {
    const macCreate = createRunbookTools(store, { platform: "darwin" })
      .find((tool) => tool.descriptor.name === "runbook.create");
    const windowsCreate = createRunbookTools(store, { platform: "win32" })
      .find((tool) => tool.descriptor.name === "runbook.create");
    const macGuidance = macCreate.parameters.properties.cells.items.properties.language.description;
    const windowsGuidance = windowsCreate.parameters.properties.cells.items.properties.language.description;

    assert.doesNotMatch(macGuidance, /powershell|pwsh/iu);
    assert.match(windowsGuidance, /pwsh/iu);
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("migrations 13 and 14 separate execution revisions and remove lifecycle status", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-runbook-migration-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const databasePath = getDefaultMemoryDatabasePath(workspaceRoot);
  const context = { sessionId: "session_migration", workspaceId: "workspace_migration", workspaceName: "Migration" };
  let store = new RunbookStore(databasePath, layout, context);
  try {
    const created = store.create({ title: "Historical runbook", purpose: "Classify old execution churn." }, { provider: "openai", model: "gpt-5.6" });
    const appended = store.append({
      id: created.runbook.id,
      expectedRevision: 1,
      cells: [{ kind: "markdown", source: "Content update", features: ["runtime"] }],
    }, { provider: "openai", model: "gpt-5.6" });
    store.close();

    const database = new DatabaseSync(databasePath);
    try {
      database.prepare("UPDATE app_server_runbooks SET revision = 3, content_revision = 1 WHERE id = ?").run(appended.runbook.id);
      database.prepare(`INSERT INTO app_server_artifact_revisions (
        artifact_kind, artifact_id, workspace_id, session_id, revision, created_at, revision_kind
      ) VALUES ('runbook', ?, ?, ?, 3, ?, 'content')`).run(
        appended.runbook.id,
        context.workspaceId,
        context.sessionId,
        "2026-08-20T00:00:00.000Z",
      );
      database.exec("ALTER TABLE app_server_runbooks ADD COLUMN status TEXT NOT NULL DEFAULT 'active'");
      database.prepare("DELETE FROM schema_migrations WHERE component = 'app_server_core' AND version >= 13").run();
    } finally {
      database.close();
    }

    store = new RunbookStore(databasePath, layout, context);
    const migrated = store.get(appended.runbook.id);
    assert.equal(migrated.contentRevision, 2);
    const migratedDatabase = new DatabaseSync(databasePath, { readOnly: true });
    try {
      assert.equal(migratedDatabase.prepare("PRAGMA table_info(app_server_runbooks)").all().some((column) => column.name === "status"), false);
      assert.deepEqual(
        migratedDatabase.prepare(`SELECT revision, revision_kind FROM app_server_artifact_revisions
          WHERE artifact_kind = 'runbook' AND artifact_id = ? ORDER BY revision`).all(appended.runbook.id).map((row) => ({ ...row })),
        [
          { revision: 1, revision_kind: "content" },
          { revision: 2, revision_kind: "content" },
          { revision: 3, revision_kind: "execution" },
        ],
      );
    } finally {
      migratedDatabase.close();
    }
  } finally {
    try { store.close(); } catch { /* already closed before migration replay */ }
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("runbook execution records cell status, output, and duration through the shell boundary", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-runbook-execution-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const store = new RunbookStore(
    getDefaultMemoryDatabasePath(workspaceRoot),
    layout,
    { sessionId: "session_exec", workspaceId: "workspace_exec", workspaceName: "Execution" },
  );
  const calls = [];
  const updates = [];
  const shellTool = {
    descriptor: {
      name: "shell.run",
      description: "fixture",
      actionClasses: ["experiment"],
      sideEffects: "process",
      requiredPermissions: ["process:spawn"],
    },
    async execute(action, context) {
      calls.push({ input: action.input, context: context.runbookContext });
      return {
        action,
        status: "complete",
        startedAt: new Date().toISOString(),
        completedAt: new Date().toISOString(),
        summary: "complete",
        output: { stdout: "proof passed\n", stderr: "", exitCode: 0 },
        followUpActions: [],
      };
    },
  };
  try {
    const created = store.create({
      title: "Proof sequence",
      purpose: "Run one bounded and repeatable proof command.",
      cells: [{
        kind: "code",
        language: "sh",
        source: "printf 'proof passed\\n'",
        features: ["runtime"],
        executor: { kind: "host", timeoutSeconds: 180 },
      }],
    });
    const execute = createRunbookExecutor({
      store,
      shellTool,
      onUpdate: (update) => updates.push(update),
    });
    await assert.rejects(
      execute({ runbookId: created.runbook.id, proofTarget: "device" }),
      /deviceOs/,
    );
    const executionTool = createRunbookExecutionTool(execute);
    const executionResult = await executionTool.execute({
      id: "execute_proof",
      toolName: "runbook.run",
      actionClass: "experiment",
      input: { id: created.runbook.id, proofTarget: "device", deviceOs: "iOS 27.0" },
    });

    assert.equal(calls.length, 1);
    assert.equal(calls[0].input.timeoutMs, 180_000);
    assert.equal(calls[0].context.runbookId, created.runbook.id);
    assert.match(calls[0].context.runId, /^runbook_run_/);
    assert.match(calls[0].context.cellId, /^cell-/);
    assert.equal(updates.at(-1).status, "succeeded");
    assert.equal(executionResult.status, "complete");
    assert.equal(executionResult.output.status, "succeeded");
    assert.equal(executionResult.output.title, "Proof sequence");
    assert.equal(executionResult.output.runId, calls[0].context.runId);
    assert.match(executionResult.summary, new RegExp(calls[0].context.runId));

    const artifact = listResearchStorageArtifacts(layout, { kind: "runbook" })
      .find((candidate) => candidate.id === created.runbook.artifactId);
    assert.ok(artifact);
    const notebook = JSON.parse(await readFile(artifact.path, "utf8"));
    const codeCell = notebook.cells[1];
    assert.equal(codeCell.metadata.beale.executor.timeoutSeconds, 180);
    assert.equal(codeCell.execution_count, 1);
    assert.equal(codeCell.outputs[0].text.join(""), "proof passed\n");
    assert.equal(codeCell.metadata.beale.latestRun.status, "succeeded");
    assert.equal(codeCell.metadata.beale.latestRun.proofTarget, "device");
    assert.equal(codeCell.metadata.beale.latestRun.deviceOs, "iOS 27.0");
    assert.equal(typeof codeCell.metadata.beale.latestRun.durationMs, "number");
    assert.equal(notebook.metadata.beale.latestRun.status, "succeeded");
    assert.equal(notebook.metadata.beale.latestRun.proofTarget, "device");
    assert.equal(notebook.metadata.beale.latestRun.deviceOs, "iOS 27.0");
    assert.equal(typeof notebook.metadata.beale.latestRun.durationMs, "number");
    const executed = store.get(created.runbook.id);
    assert.equal(executed.contentRevision, 1);
    assert.ok(executed.revision > executed.contentRevision);
    assert.equal(executed.execution.runCount, 1);
    assert.equal(executed.execution.completedRunCount, 1);
    assert.equal(executed.execution.executedCellCount, 1);
    assert.equal(executed.execution.latest.status, "succeeded");
    assert.equal(executed.execution.latest.runId, calls[0].context.runId);
    assert.equal(executed.execution.latestSuccessfulRunId, calls[0].context.runId);

    const database = new DatabaseSync(getDefaultMemoryDatabasePath(workspaceRoot), { readOnly: true });
    try {
      assert.equal(database.prepare(`SELECT COUNT(*) AS count FROM app_server_artifact_revisions
        WHERE artifact_kind = 'runbook' AND artifact_id = ?`).get(created.runbook.id).count, 1);
    } finally {
      database.close();
    }
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("runbook execution plans support inclusive cell ranges and resume-from-here selection", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-runbook-range-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const store = new RunbookStore(
    getDefaultMemoryDatabasePath(workspaceRoot),
    layout,
    { sessionId: "session_range", workspaceId: "workspace_range", workspaceName: "Range" },
  );
  try {
    const created = store.create({
      title: "Resume sequence",
      purpose: "Prove that a repaired late step can resume without repeating the prefix.",
      cells: [
        { kind: "code", language: "sh", source: "printf 'one\\n'", features: ["setup"] },
        { kind: "markdown", source: "Inspect the first result.", features: ["runtime"] },
        { kind: "code", language: "sh", source: "printf 'two\\n'", features: ["runtime"] },
        { kind: "code", language: "sh", source: "printf 'three\\n'", features: ["cleanup"] },
      ],
    });
    const codeCells = store.get(created.runbook.id).cells.filter((cell) => cell.kind === "code");
    assert.equal(codeCells.length, 3);
    assert.deepEqual(
      store.executionPlan(created.runbook.id, { startCellId: codeCells[1].id }).map((cell) => cell.id),
      [codeCells[1].id, codeCells[2].id],
    );
    assert.deepEqual(
      store.executionPlan(created.runbook.id, { endCellId: codeCells[1].id }).map((cell) => cell.id),
      [codeCells[0].id, codeCells[1].id],
    );
    assert.deepEqual(
      store.executionPlan(created.runbook.id, { startCellId: codeCells[1].id, endCellId: codeCells[2].id }).map((cell) => cell.id),
      [codeCells[1].id, codeCells[2].id],
    );
    assert.throws(
      () => store.executionPlan(created.runbook.id, { startCellId: codeCells[2].id, endCellId: codeCells[0].id }),
      /must precede/,
    );
    assert.throws(
      () => store.executionPlan(created.runbook.id, { cellId: codeCells[0].id, startCellId: codeCells[1].id }),
      /cannot be combined/,
    );
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("runbook feature toggles deactivate unmatched cells and preserve phase labels", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-runbook-features-"));
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
  const store = new RunbookStore(
    getDefaultMemoryDatabasePath(workspaceRoot),
    layout,
    { sessionId: "session_features", workspaceId: "workspace_features", workspaceName: "Features" },
  );
  try {
    const created = store.create({
      title: "Feature-selected workflow",
      purpose: "Keep setup, proof, and teardown in one executable workflow.",
      cells: [
        { kind: "code", language: "sh", source: "printf 'setup\\n'", features: ["setup"] },
        { kind: "code", language: "sh", source: "printf 'proof\\n'", features: ["runtime", "variant-a"] },
        { kind: "code", language: "sh", source: "printf 'cleanup\\n'", features: ["cleanup"] },
      ],
    });
    const page = store.get(created.runbook.id);
    const codeCells = page.cells.filter((cell) => cell.kind === "code");
    const configured = store.configure({
      id: created.runbook.id,
      expectedRevision: created.runbook.revision,
      enabledFeatures: ["variant-a"],
      cellFeatures: [{ cellId: codeCells[1].id, features: ["runtime", "variant-a"] }],
    });

    assert.deepEqual(configured.runbook.enabledFeatures, ["variant-a"]);
    assert.deepEqual(store.executionPlan(created.runbook.id).map((cell) => cell.id), [codeCells[1].id]);
    assert.equal(store.get(created.runbook.id).cells.find((cell) => cell.id === codeCells[0].id).active, false);
    assert.throws(() => store.executionPlan(created.runbook.id, { cellId: codeCells[0].id }), /deactivated/);
    assert.throws(
      () => store.configure({
        id: created.runbook.id,
        expectedRevision: configured.runbook.revision,
        enabledFeatures: ["runtime"],
        cellFeatures: [{ cellId: codeCells[1].id, features: ["variant-b"] }],
      }),
      /must include setup, runtime, or cleanup/,
    );
  } finally {
    store.close();
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});
