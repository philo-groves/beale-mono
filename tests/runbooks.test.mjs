import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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
    { sessionId: "run_tools", workspaceId: "workspace_tools", workspaceName: "Tools" },
  );
  const registry = createResearchToolRegistry(createRunbookTools(store));
  try {
    const descriptors = registry.listDescriptors();
    assert.deepEqual(descriptors.map((tool) => tool.name), ["runbook.list", "runbook.get", "runbook.create", "runbook.append"]);
    assert.equal("statuses" in descriptors.find((tool) => tool.name === "runbook.list").inputSchema.properties, false);
    assert.equal("status" in descriptors.find((tool) => tool.name === "runbook.create").inputSchema.properties, false);
    assert.equal("status" in descriptors.find((tool) => tool.name === "runbook.append").inputSchema.properties, false);
    const created = await registry.execute({
      id: "create_runbook",
      actionClass: "synthesize",
      toolName: "runbook.create",
      input: { title: "Crash triage", purpose: "Repeatable crash collection and classification." },
    });
    assert.equal(created.result.status, "complete");
    assert.equal(created.result.artifactRefs[0].kind, "runbook");

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
      cells: [{ kind: "markdown", source: "Content update" }],
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
  const contexts = [];
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
      contexts.push(context.runbookContext);
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
      cells: [{ kind: "code", language: "sh", source: "printf 'proof passed\\n'" }],
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

    assert.equal(contexts.length, 1);
    assert.equal(contexts[0].runbookId, created.runbook.id);
    assert.match(contexts[0].runId, /^runbook_run_/);
    assert.match(contexts[0].cellId, /^cell-/);
    assert.equal(updates.at(-1).status, "succeeded");
    assert.equal(executionResult.status, "complete");
    assert.equal(executionResult.output.status, "succeeded");
    assert.equal(executionResult.output.title, "Proof sequence");
    assert.equal(executionResult.output.runId, contexts[0].runId);
    assert.match(executionResult.summary, new RegExp(contexts[0].runId));

    const artifact = listResearchStorageArtifacts(layout, { kind: "runbook" })[0];
    const notebook = JSON.parse(await readFile(artifact.path, "utf8"));
    const codeCell = notebook.cells[1];
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
    assert.equal(executed.execution.latest.runId, contexts[0].runId);
    assert.equal(executed.execution.latestSuccessfulRunId, contexts[0].runId);

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
        { kind: "code", language: "sh", source: "printf 'one\\n'" },
        { kind: "markdown", source: "Inspect the first result." },
        { kind: "code", language: "sh", source: "printf 'two\\n'" },
        { kind: "code", language: "sh", source: "printf 'three\\n'" },
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
