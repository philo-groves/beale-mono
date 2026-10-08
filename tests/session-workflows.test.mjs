import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';
import { AppServerSessionStore, SessionWorkflowStore, RunbookStore, createResearchStorageLayout, ensureResearchStorageLayout, createRunbookExecutor, createSessionWorkflowTool, ensureSessionWorkflowRunbook, requireAssignedSessionWorkflowDisposition, sessionWorkflowInstructions, verifySessionWorkflowCodeRun } from '../packages/research-agent/dist/index.js';
import { invokeAppServerProtocol } from '../app-server/dist/appServerProtocolClient.js';
import { APP_SERVER_SESSION_LAUNCH_VERSION, decodeAppServerSessionLaunchRequest } from '../packages/app-server-runtime/dist/protocol.js';

test('assigned repository auditor tracks exact line coverage across scopes and changed files', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const repository = join(root, 'example-repository');
  const modulePath = join(repository, 'module');
  mkdirSync(modulePath, { recursive: true });
  const first = join(repository, 'main.c');
  const second = join(modulePath, 'helper.ts');
  const unusual = join(modulePath, 'script.custom');
  writeFileSync(first, 'int first;\nint second;\nint third;\n');
  writeFileSync(second, 'export const value = 1;\n');
  writeFileSync(unusual, 'custom source line\n');
  const git = (args) => {
    const result = spawnSync('git', ['-C', repository, ...args], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
  };
  git(['init', '-q']);
  git(['add', '.']);
  const store = new SessionWorkflowStore(join(root, 'memory.sqlite'), 'workspace-example');
  try {
    assert.equal(store.list()[0].id, 'beale.repository-auditor');
    assert.throws(() => store.assign('session-example', 'beale.repository-auditor', {}), /required/u);
    const assigned = store.assign('session-example', 'beale.repository-auditor', { systems: 'example repository\nmodule' });
    assert.equal(assigned.stepIndex, 0);
    assert.equal(store.assign('session-example', 'beale.repository-auditor', { systems: 'example repository\nmodule' }).stepIndex, 0);
    assert.match(sessionWorkflowInstructions(store, 'session-example'), /system-to-path mapping/u);
    assert.throws(() => requireAssignedSessionWorkflowDisposition(store, 'session-example', { outcome: 'blocked' }), /Use workflow.progress/u);
    assert.throws(() => store.advance('session-example', 'scope', 'Mapped example systems.'), /Map every named system/u);
    const tool = createSessionWorkflowTool(store, 'session-example', [repository]);
    const incompleteScope = await tool.execute({ id: 'incomplete-scope-example', actionClass: 'inspect', toolName: 'workflow.progress',
      input: { action: 'scope', targets: [{ system: 'example repository', path: repository }] } });
    assert.equal(incompleteScope.status, 'error');
    assert.match(incompleteScope.error.message, /every operator-named system/u);
    assert.throws(() => store.declareAuditTargets('session-example', [
      { system: 'example repository', path: root }, { system: 'module', path: modulePath }
    ], [repository]), /outside known repository roots/u);
    const scoped = await tool.execute({ id: 'scope-example', actionClass: 'inspect', toolName: 'workflow.progress',
      input: { action: 'scope', targets: [{ system: 'example repository', path: repository }, { system: 'module', path: modulePath }] } });
    assert.equal(scoped.status, 'complete');
    assert.equal(scoped.output.auditTargets.length, 2);
    assert.throws(() => store.inventory('session-example', [repository], [repository]), /Complete the scope step/u);
    const scopedStep = await tool.execute({ id: 'advance-scope-example', actionClass: 'analyze', toolName: 'workflow.progress',
      input: { action: 'advance', stepId: 'scope', note: 'Mapped both named example systems to repository paths.' } });
    assert.equal(scopedStep.status, 'complete');
    assert.equal(scopedStep.output.completedSteps[0].stepId, 'scope');
    assert.equal(tool.descriptor.name, 'workflow.progress');
    assert.equal(tool.descriptor.inputSchema.properties.action.enum.includes('inspect'), true);
    assert.equal(tool.descriptor.inputSchema.properties.action.enum.includes('cover'), false);
    assert.equal((await tool.execute({ id: 'status-example', actionClass: 'inspect', toolName: 'workflow.progress',
      input: { action: 'status' } })).status, 'complete');
    assert.doesNotThrow(() => requireAssignedSessionWorkflowDisposition(store, 'session-example', { outcome: 'blocked' }));
    assert.throws(() => requireAssignedSessionWorkflowDisposition(store, 'session-example', { outcome: 'objective_achieved' }), /coverage/u);
    const outside = await tool.execute({ id: 'outside-example', actionClass: 'inspect', toolName: 'workflow.progress',
      input: { action: 'inventory', paths: [root] } });
    assert.equal(outside.status, 'error');
    assert.match(outside.error.message, /must match the declared/u);
    assert.throws(() => store.advance('session-example', 'inventory', 'Inventoried example source.'), /Inventory/u);
    store.inventory('session-example', [repository], [repository]);
    assert.throws(() => store.advance('session-example', 'inventory', 'Inventoried example source.'), /declared path for module/u);
    const inventory = store.inventory('session-example', [modulePath], [repository]);
    assert.equal(inventory.fileCount, 3);
    assert.equal(inventory.scopePaths.length, 2);
    store.advance('session-example', 'inventory', 'Inventoried both declared paths.');
    store.advance('session-example', 'map', 'Mapped example entry points.');
    const inspected = store.inspect('session-example', [{ path: first, startLine: 1, endLine: 2 }]);
    assert.deepEqual(inspected.excerpts[0].lines.map((line) => line.text), ['int first;', 'int second;']);
    assert.deepEqual(store.getAssignment('session-example').nextUncovered.find((file) => file.path === realpathSync(first)).nextRange,
      { startLine: 3, endLine: 3 });
    store.inspect('session-example', [{ path: first, startLine: 2, endLine: 3 }]);
    assert.equal(store.getAssignment('session-example').coveredFileCount, 1);
    assert.throws(() => store.advance('session-example', 'review', 'Reviewed example source.'), /fully covered/u);
    store.inspect('session-example', [{ path: second, startLine: 1, endLine: 1 }]);
    assert.equal(store.getAssignment('session-example').coveredFileCount, 2);
    store.inspect('session-example', [{ path: unusual, startLine: 1, endLine: 1 }]);
    writeFileSync(second, 'export const value = 1;\nexport const next = 2;\n');
    assert.throws(() => store.inspect('session-example', [{ path: second, startLine: 1, endLine: 1 }]), /changed/u);
    assert.equal(store.inventory('session-example', [modulePath], [repository]).coveredFileCount, 2);
    store.inspect('session-example', [{ path: second, startLine: 1, endLine: 2 }]);
    const unsupportedCover = await tool.execute({ id: 'cover-example', actionClass: 'analyze', toolName: 'workflow.progress',
      input: { action: 'cover', path: first, startLine: 1, endLine: 3 } });
    assert.equal(unsupportedCover.status, 'error');
    assert.match(unsupportedCover.error.message, /Use workflow.progress inspect/u);
    const batched = await tool.execute({ id: 'batch-example', actionClass: 'analyze', toolName: 'workflow.progress',
      input: { action: 'inspect', ranges: [{ path: first, startLine: 1, endLine: 3 }, { path: second, startLine: 1, endLine: 2 }] } });
    assert.equal(batched.status, 'complete');
    assert.equal(batched.modelOutput.excerpts.length, 2);
    assert.equal(store.getAssignment('session-example').coveredFileCount, 3);
    store.advance('session-example', 'review', 'Inspected all inventoried lines.');
    store.advance('session-example', 'validate', 'No candidate issue in example source.');
    assert.equal(store.advance('session-example', 'report', 'Summarized example coverage.').completedAt !== null, true);
    assert.doesNotThrow(() => requireAssignedSessionWorkflowDisposition(store, 'session-example', { outcome: 'objective_achieved' }));
    writeFileSync(unusual, 'custom source line\nsecond line\n');
    assert.throws(() => requireAssignedSessionWorkflowDisposition(store, 'session-example', { outcome: 'objective_achieved' }), /uncovered/u);
    const reopened = store.getAssignment('session-example');
    assert.equal(reopened.stepIndex, 3);
    assert.equal(reopened.completedAt, null);
    store.inspect('session-example', [{ path: unusual, startLine: 1, endLine: 2 }]);
    store.advance('session-example', 'review', 'Reinspected changed example source.');
    store.advance('session-example', 'validate', 'Rechecked example candidates.');
    store.advance('session-example', 'report', 'Updated example coverage summary.');
    const added = join(modulePath, 'new-source.rs');
    writeFileSync(added, 'fn example() {}\n');
    assert.throws(() => requireAssignedSessionWorkflowDisposition(store, 'session-example', { outcome: 'objective_achieved' }), /uncovered/u);
    assert.equal(store.getAssignment('session-example').fileCount, 4);
    assert.throws(() => store.assign('session-example', 'beale.repository-auditor', { systems: 'different' }), /cannot change/u);
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('custom workflow snapshots ordered steps and configuration for a session', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const store = new SessionWorkflowStore(join(root, 'memory.sqlite'), 'workspace-example');
  try {
    const definition = store.create({ title: 'Example review', description: 'Review an example system.',
      fields: [{ id: 'component', label: 'Component', description: 'Name the component to review.', required: true, multiline: true }],
      steps: [{ id: 'inspect', title: 'Inspect', instructions: 'Read the source.' },
        { id: 'report', title: 'Report', instructions: 'Summarize the result.' }] });
    assert.equal(store.list().length, 2);
    assert.equal(definition.fields[0].multiline, true);
    assert.equal(definition.fields[0].description, 'Name the component to review.');
    const otherWorkspace = new SessionWorkflowStore(join(root, 'memory.sqlite'), 'workspace-other');
    try { assert.equal(otherWorkspace.list().some((item) => item.id === definition.id), true); }
    finally { otherWorkspace.close(); }
    assert.throws(() => store.assign('session-example', definition.id, {}), /required/u);
    const assignment = store.assign('session-example', definition.id, { component: 'example component' });
    assert.match(sessionWorkflowInstructions(store, 'session-example'), /Name the component to review\./u);
    assert.equal(assignment.definition.steps[0].id, 'inspect');
    assert.deepEqual(createSessionWorkflowTool(store, 'session-example', []).descriptor.inputSchema.properties.action.enum,
      ['status', 'advance']);
    assert.throws(() => store.advance('session-example', 'inspect', ''), /Step completion note/u);
    assert.throws(() => store.advance('session-example', 'report', 'Premature report.'), /current/u);
    store.advance('session-example', 'inspect', 'Read the example source.');
    const completed = store.advance('session-example', 'report', 'Summarized the example.');
    assert.equal(completed.completedAt !== null, true);
    assert.deepEqual(completed.completedSteps.map((step) => step.note), ['Read the example source.', 'Summarized the example.']);
    const edited = store.update({ id: definition.id, expectedRevision: definition.revision,
      title: 'Revised example review', description: 'Review an example module.', fields: definition.fields,
      notebook: { ...definition.notebook, cells: [definition.notebook.cells[1], definition.notebook.cells[0]] } });
    assert.equal(edited.revision, 2);
    assert.deepEqual(edited.steps.map((step) => step.id), ['report', 'inspect']);
    assert.deepEqual(edited.notebook.cells.map((cell) => cell.id), ['report', 'inspect']);
    assert.equal(store.getAssignment('session-example').definition.steps[0].id, 'inspect');
    assert.equal(store.assign('session-new-example', edited.id, { component: 'example module' }).definition.revision, 2);
    assert.throws(() => store.update({ ...edited, expectedRevision: 1 }), /changed; reload/u);
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('workflow run history is workspace-scoped and retains each assigned revision', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const databasePath = join(root, 'memory.sqlite');
  const store = new SessionWorkflowStore(databasePath, 'workspace-example');
  try {
    const workflow = store.create({ title: 'Example review', description: 'Review source.',
      fields: [{ id: 'component', label: 'Component', required: true }],
      steps: [{ id: 'inspect', title: 'Inspect', instructions: 'Read source.' }] });
    store.assign('session-first-example', workflow.id, { component: 'first component' });
    store.advance('session-first-example', 'inspect', 'Read source.');
    store.update({ ...workflow, expectedRevision: workflow.revision, title: 'Revised review' });
    store.assign('session-second-example', workflow.id, { component: 'second component' });
    const runs = store.listRuns(workflow.id);
    assert.deepEqual(runs.map((run) => run.sessionId), ['session-second-example', 'session-first-example']);
    assert.deepEqual(runs.map((run) => run.revision), [2, 1]);
    assert.deepEqual(runs.map((run) => run.stepIndex), [0, 1]);
    assert.ok(runs[0].startedAt);
    assert.equal(store.getAssignment(runs[0].sessionId).values.component, 'second component');
    assert.equal(store.getAssignment(runs[1].sessionId).values.component, 'first component');
    const otherWorkspace = new SessionWorkflowStore(databasePath, 'workspace-other');
    try { assert.deepEqual(otherWorkspace.listRuns(workflow.id), []); }
    finally { otherWorkspace.close(); }
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('workflow run history migrates earlier assignment rows', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const databasePath = join(root, 'memory.sqlite');
  const database = new DatabaseSync(databasePath);
  try {
    database.exec(`CREATE TABLE beale_session_workflow_assignments (
      workspace_id TEXT NOT NULL, session_id TEXT NOT NULL, definition_json TEXT NOT NULL,
      values_json TEXT NOT NULL, step_index INTEGER NOT NULL DEFAULT 0, completed_at TEXT,
      PRIMARY KEY (workspace_id, session_id))`);
    database.prepare(`INSERT INTO beale_session_workflow_assignments
      (workspace_id, session_id, definition_json, values_json) VALUES (?, ?, ?, ?)`).run(
      'workspace-example', 'session-legacy-example', JSON.stringify({ id: 'workflow-example', title: 'Legacy review',
        description: 'Review source.', kind: 'custom', revision: 1, fields: [],
        steps: [{ id: 'inspect', title: 'Inspect', instructions: 'Read source.' }] }), '{}');
  } finally { database.close(); }
  const store = new SessionWorkflowStore(databasePath, 'workspace-example');
  try {
    assert.deepEqual(store.listRuns('workflow-example').map((run) => run.sessionId), ['session-legacy-example']);
    assert.equal(store.listRuns('workflow-example')[0].cellCount, 1);
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('workflow notebooks persist markdown and language-tagged code as nbformat 4 cells', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const store = new SessionWorkflowStore(join(root, 'memory.sqlite'), 'workspace-example');
  const notebook = { nbformat: 4, nbformat_minor: 5, metadata: { beale: { kind: 'session_workflow' } }, cells: [
    { id: 'context', cell_type: 'markdown', metadata: { beale: { title: 'Context' } }, source: ['Review the example module.'] },
    { id: 'python_check', cell_type: 'code', metadata: { beale: { title: 'Python check', language: 'python3' } }, source: ['print("example")\n'] },
    { id: 'shell_check', cell_type: 'code', metadata: { beale: { title: 'Shell check', language: 'sh' } }, source: ['printf example\n'] }
  ] };
  try {
    const definition = store.create({ title: 'Example notebook', description: 'Check an example module.', fields: [], notebook });
    assert.equal(definition.notebook.nbformat, 4);
    assert.equal(definition.notebook.nbformat_minor, 5);
    assert.deepEqual(definition.notebook.cells.map((cell) => cell.cell_type), ['markdown', 'code', 'code']);
    assert.deepEqual(definition.notebook.cells.map((cell) => cell.metadata.beale.language ?? null), [null, 'python3', 'sh']);
    assert.deepEqual(definition.notebook.cells[1].outputs, []);
    assert.equal(definition.notebook.cells[1].execution_count, null);
    assert.deepEqual(store.getDefinition(definition.id).notebook, definition.notebook);
    const assignment = store.assign('session-example', definition.id, {});
    assert.deepEqual(assignment.definition.notebook, definition.notebook);
    assert.match(sessionWorkflowInstructions(store, 'session-example'), /python3 code cell/u);
    assert.match(sessionWorkflowInstructions(store, 'session-example'), /runbook\.run/u);
    store.attachRunbook('session-example', 'runbook-example');
    store.advance('session-example', 'context', 'Read the example context.');
    assert.throws(() => store.advance('session-example', 'python_check', 'Checked example output.'), /successful runId/u);
    const codeCell = store.getAssignment('session-example').definition.notebook.cells[1];
    const execution = { status: 'succeeded', sessionId: 'session-example', selectedCellIds: ['runbook-cell-example'],
      cells: [{ source: codeCell.source.join(''), language: 'python3', result: { status: 'succeeded' } }] };
    const runbooks = { get: () => ({ cells: [{ id: 'runbook-cell-example', kind: 'code', source: codeCell.source.join(''), language: 'python3' }] }),
      getExecution: () => execution };
    assert.throws(() => store.advance('session-example', 'python_check', 'Checked example output.', 'run-example',
      (current, cell, runId) => verifySessionWorkflowCodeRun({ ...runbooks, getExecution: () => ({ ...execution, status: 'failed' }) }, current, cell, runId)), /successful execution/u);
    assert.throws(() => store.advance('session-example', 'python_check', 'Checked example output.', 'run-example',
      (current, cell, runId) => verifySessionWorkflowCodeRun({ ...runbooks, getExecution: () => ({ ...execution, cells: [{ ...execution.cells[0], source: 'print("changed")' }] }) }, current, cell, runId)), /exact workflow code cell/u);
    store.advance('session-example', 'python_check', 'Checked example output.', 'run-example',
      (current, cell, runId) => verifySessionWorkflowCodeRun(runbooks, current, cell, runId));
    assert.match(store.getAssignment('session-example').completedSteps[1].note, /run-example/u);
    assert.throws(() => store.create({ title: 'Invalid language', description: 'Example.', fields: [],
      notebook: { ...notebook, cells: [{ ...notebook.cells[1], metadata: { beale: { title: 'Invalid', language: 'rust' } } }] }
    }), /Unsupported workflow code language/u);
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('saved instruction-only workflows hydrate as markdown notebooks', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const databasePath = join(root, 'memory.sqlite');
  const store = new SessionWorkflowStore(databasePath, 'workspace-example');
  try {
    const created = store.create({ title: 'Legacy example', description: 'Review example source.', fields: [],
      steps: [{ id: 'inspect', title: 'Inspect', instructions: 'Read example source.' }] });
    const database = new DatabaseSync(databasePath);
    try { database.prepare('UPDATE beale_session_workflow_definitions SET definition_json = ? WHERE id = ?')
      .run(JSON.stringify({ ...created, notebook: undefined }), created.id); }
    finally { database.close(); }
    const restored = store.getDefinition(created.id);
    assert.equal(restored.notebook.nbformat, 4);
    assert.equal(restored.notebook.cells[0].cell_type, 'markdown');
    assert.equal(restored.notebook.cells[0].source.join(''), 'Read example source.');
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('code workflow materializes and verifies execution of its session runbook', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const databasePath = join(root, 'memory.sqlite');
  const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot: root, databasePath, artifactDirectoryPath: join(root, 'artifacts') }));
  const context = { sessionId: 'session-example', workspaceId: 'workspace-example', workspaceName: 'Example workspace', subjectId: 'subject-example', subjectName: 'Example subject' };
  const workflows = new SessionWorkflowStore(databasePath, context.workspaceId);
  const runbooks = new RunbookStore(databasePath, layout, context);
  try {
    const notebook = { nbformat: 4, nbformat_minor: 5, metadata: { beale: { kind: 'session_workflow' } }, cells: [
      { id: 'explain', cell_type: 'markdown', metadata: { beale: { title: 'Explain' } }, source: ['Explain the example check.'] },
      { id: 'check', cell_type: 'code', metadata: { beale: { title: 'Check', language: 'python3' } }, source: ['\n', 'print("example")\n'] }
    ] };
    const definition = workflows.create({ title: 'Example notebook', description: 'Check example source.', fields: [], notebook });
    workflows.assign(context.sessionId, definition.id, {});
    const attached = ensureSessionWorkflowRunbook(workflows, runbooks, context.sessionId);
    assert.ok(attached.runbookId);
    const page = runbooks.get(attached.runbookId, { limit: 10 });
    assert.equal(page.sessionId, context.sessionId);
    assert.equal(page.cellCount, 3);
    assert.deepEqual(page.cells.map((cell) => cell.kind), ['markdown', 'markdown', 'code']);
    assert.equal(page.cells[2].language, 'python3');
    assert.equal(page.cells[2].source, '\nprint("example")\n');
    assert.equal(ensureSessionWorkflowRunbook(workflows, runbooks, context.sessionId).runbookId, attached.runbookId);
    assert.equal(runbooks.list().length, 1);
    const calls = [];
    const shellTool = { descriptor: { name: 'shell.run', description: 'fixture', actionClasses: ['experiment'], sideEffects: 'process', requiredPermissions: [] },
      async execute(action) {
        calls.push(action.input);
        return { action, status: 'complete', startedAt: new Date().toISOString(), completedAt: new Date().toISOString(),
          summary: 'complete', output: { stdout: 'example\n', exitCode: 0 }, followUpActions: [] };
      } };
    const run = await createRunbookExecutor({ store: runbooks, shellTool })({ runbookId: attached.runbookId, cellId: page.cells[2].id, proofTarget: 'localhost' });
    assert.equal(run.status, 'succeeded');
    assert.equal(calls[0].utility, 'python3');
    assert.equal(calls[0].args[1], '\nprint("example")\n');
    workflows.advance(context.sessionId, 'explain', 'Read the notebook context.');
    const completed = workflows.advance(context.sessionId, 'check', 'Executed the Python check.', run.runId,
      (current, cell, runId) => verifySessionWorkflowCodeRun(runbooks, current, cell, runId));
    assert.equal(completed.completedAt !== null, true);
    assert.match(completed.completedSteps[1].note, /runbook_run_/u);
  } finally {
    runbooks.close();
    workflows.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('built-in auditor cells can be edited without changing required structure or existing assignments', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const store = new SessionWorkflowStore(join(root, 'memory.sqlite'), 'workspace-example');
  try {
    const builtIn = store.getDefinition('beale.repository-auditor');
    store.assign('session-old-example', builtIn.id, { systems: 'example module' });
    assert.throws(() => store.update({ id: builtIn.id, expectedRevision: builtIn.revision,
      title: builtIn.title, description: builtIn.description, fields: builtIn.fields, steps: builtIn.steps.slice(1) }), /fixed structure/u);
    const edited = store.update({ id: builtIn.id, expectedRevision: builtIn.revision,
      title: 'Repository source review', description: builtIn.description,
      fields: [...builtIn.fields.map((field) => ({ ...field, label: field.id === 'focus' ? 'Priority areas' : field.label })),
        { id: 'notes', label: 'Review notes', description: 'Optional context for the auditor.', required: false }],
      steps: builtIn.steps.map((step) => step.id === 'map' ? { ...step, instructions: 'Map example interfaces and boundaries.' } : step) });
    assert.equal(edited.revision, builtIn.revision + 1);
    assert.equal(edited.fields[2].description, 'Optional context for the auditor.');
    assert.equal(store.list()[0].title, 'Repository source review');
    assert.equal(store.getDefinition(builtIn.id).steps.find((step) => step.id === 'map').instructions, 'Map example interfaces and boundaries.');
    assert.equal(store.getAssignment('session-old-example').definition.revision, builtIn.revision);
    assert.equal(store.getAssignment('session-old-example').definition.fields.length, 2);
    assert.equal(store.assign('session-new-example', builtIn.id, { systems: 'example module' }).definition.revision, edited.revision);
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('auditor pages a large uncovered inventory without returning all file paths at once', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const repository = join(root, 'example-repository');
  mkdirSync(repository);
  for (let index = 0; index < 75; index += 1) {
    writeFileSync(join(repository, `unit-${String(index).padStart(3, '0')}.c`), `int unit_${index};\n`);
  }
  assert.equal(spawnSync('git', ['-C', repository, 'init', '-q']).status, 0);
  assert.equal(spawnSync('git', ['-C', repository, 'add', '.']).status, 0);
  const store = new SessionWorkflowStore(join(root, 'memory.sqlite'), 'workspace-example');
  try {
    store.assign('session-example', 'beale.repository-auditor', { systems: 'all example units' });
    store.declareAuditTargets('session-example', [{ system: 'all example units', path: repository }], [repository]);
    store.advance('session-example', 'scope', 'Mapped the example repository.');
    const first = store.inventory('session-example', [repository], [repository]);
    assert.equal(first.fileCount, 75);
    assert.equal(first.nextUncovered.length, 50);
    assert.ok(first.nextUncoveredCursor);
    const next = store.getAssignment('session-example', first.nextUncoveredCursor);
    assert.equal(next.nextUncovered.length, 25);
    assert.equal(next.nextUncoveredCursor, null);
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('auditor rejects oversized inspection output without crediting coverage', () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const repository = join(root, 'example-repository');
  mkdirSync(repository);
  const large = join(repository, 'large-source.js');
  writeFileSync(large, `${'x'.repeat(49 * 1024)}\n`);
  assert.equal(spawnSync('git', ['-C', repository, 'init', '-q']).status, 0);
  assert.equal(spawnSync('git', ['-C', repository, 'add', '.']).status, 0);
  const store = new SessionWorkflowStore(join(root, 'memory.sqlite'), 'workspace-example');
  try {
    store.assign('session-example', 'beale.repository-auditor', { systems: 'large example' });
    store.declareAuditTargets('session-example', [{ system: 'large example', path: repository }], [repository]);
    store.advance('session-example', 'scope', 'Mapped the large example repository.');
    store.inventory('session-example', [repository], [repository]);
    store.advance('session-example', 'inventory', 'Inventoried the large source file.');
    assert.throws(() => store.inspect('session-example', [{ path: large, startLine: 1, endLine: 1 }]), /exceeds 48 KiB/u);
    assert.equal(store.getAssignment('session-example').coveredFileCount, 0);
  } finally {
    store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('app-server workflow operations and launch DTO keep assignment optional and bounded', async () => {
  const root = mkdtempSync(join(tmpdir(), 'beale-workflow-example-'));
  const storage = { databasePath: join(root, 'memory.sqlite'), artifactDirectoryPath: root };
  try {
    const launch = { launchVersion: APP_SERVER_SESSION_LAUNCH_VERSION, launch: {
      workspaceId: 'workspace-example', promptMarkdown: 'Review the example system.'
    } };
    assert.equal(decodeAppServerSessionLaunchRequest(launch).launch.guidanceWorkflow, undefined);
    assert.equal(decodeAppServerSessionLaunchRequest({ ...launch, launch: {
      ...launch.launch, guidanceWorkflow: { id: 'beale.repository-auditor', values: { systems: 'example' } }
    } }).launch.guidanceWorkflow.id, 'beale.repository-auditor');
    assert.throws(() => decodeAppServerSessionLaunchRequest({ ...launch, launch: {
      ...launch.launch, guidanceWorkflow: { id: 'beale.repository-auditor', values: { systems: 7 } }
    } }), /Invalid workflow configuration/u);
    const list = await invokeAppServerProtocol('workflow.list', { args: [], storage, input: { workspaceId: 'workspace-example' } });
    assert.equal(list[0].id, 'beale.repository-auditor');
    const created = await invokeAppServerProtocol('workflow.create', { args: [], storage, input: {
      workspaceId: 'workspace-example', title: 'Example review', description: 'Review source.', fields: [],
      steps: [{ id: 'inspect', title: 'Inspect', instructions: 'Read source.' }]
    } });
    assert.equal(created.kind, 'custom');
    const revised = await invokeAppServerProtocol('workflow.update', { args: [], storage, input: {
      workspaceId: 'workspace-example', id: created.id, expectedRevision: created.revision,
      title: 'Example source review', description: created.description, fields: created.fields, steps: created.steps
    } });
    assert.equal(revised.revision, 2);
    const store = new SessionWorkflowStore(storage.databasePath, 'workspace-example');
    try { store.assign('session-example', created.id, {}); } finally { store.close(); }
    const sessions = new AppServerSessionStore({ databasePath: storage.databasePath });
    try {
      sessions.create({ id: 'session-example', workspaceId: 'workspace-example', attemptId: 'attempt-example',
        title: 'Example review', prompt: 'Review the example system.', model: 'example-large-model', reasoningEffort: 'high' });
    } finally { sessions.close(); }
    const assignment = await invokeAppServerProtocol('workflow.session', { args: [], storage, input: {
      workspaceId: 'workspace-example', sessionId: 'session-example'
    } });
    assert.equal(assignment.definition.id, created.id);
    const readRuns = () => invokeAppServerProtocol('workflow.runs', { args: [], storage, input: {
      workspaceId: 'workspace-example', workflowId: created.id
    } });
    const runs = await readRuns();
    assert.deepEqual(runs.map((run) => run.sessionId), ['session-example']);
    assert.equal(runs[0].sessionStatus, 'active');
    const ended = new AppServerSessionStore({ databasePath: storage.databasePath });
    try { ended.transition('session-example', { status: 'stopped', summary: 'Ended before the workflow finished.' }); }
    finally { ended.close(); }
    const endedRuns = await readRuns();
    assert.equal(endedRuns[0].sessionStatus, 'stopped');
    assert.ok(endedRuns[0].sessionEndedAt);
    const incomplete = new SessionWorkflowStore(storage.databasePath, 'workspace-example');
    try { assert.equal(incomplete.getAssignment('session-example').completedAt, null); }
    finally { incomplete.close(); }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
