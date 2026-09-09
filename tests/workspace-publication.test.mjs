import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import {
  initializeWorkspaceProject, checkpointWorkspaceResearch, importWorkspaceResearchFile,
  MemoryGraphStore, FindingStore, RunbookStore, ReportStore,
  createResearchStorageLayout, ensureResearchStorageLayout,
} from '../packages/research-agent/dist/index.js';

test('canonical snapshots isolate workspace records and imports preserve revision/evidence validation', () => {
  const directory = mkdtempSync(join(tmpdir(), 'beale-publication-example-'));
  const workspaceRoot = join(directory, 'workspace');
  const databasePath = join(directory, 'runtime', 'memory.sqlite');
  const artifactDirectoryPath = join(directory, 'runtime', 'artifacts');
  mkdirSync(workspaceRoot);
  const options = { workspaceRoot, workspaceId: 'workspace-example', databasePath, artifactDirectoryPath };
  initializeWorkspaceProject(workspaceRoot, options.workspaceId);
  const layout = ensureResearchStorageLayout(createResearchStorageLayout(options));
  const context = { workspaceId: options.workspaceId, workspaceName: 'Example Research', subjectId: 'subject-example', subjectName: 'Example Subject' };
  const graph = new MemoryGraphStore({ workspaceRoot, databasePath, context });
  const other = new MemoryGraphStore({ workspaceRoot, databasePath, context: { ...context, workspaceId: 'workspace-other', subjectId: 'subject-other' } });
  const claims = new FindingStore(graph);
  const runbooks = new RunbookStore(databasePath, layout, context);
  const reports = new ReportStore(databasePath, layout, context);
  try {
    const memory = graph.save({ type: 'invariant', title: 'Example parser boundary', summary: 'An example observation.', body: 'Original explanation.', status: 'suspected' });
    const foreign = other.save({ type: 'invariant', title: 'Other workspace knowledge', status: 'suspected' });
    const claim = claims.create({ title: 'Example hypothesis', summary: 'A synthetic hypothesis.', classification: 'security.primitive', rating: 'medium' });
    const runbook = runbooks.create({ title: 'Example procedure', purpose: 'Repeat the example.', cells: [{ kind: 'code', source: 'echo example', features: ['runtime'], language: 'shell' }] }).runbook;
    const report = reports.create({ title: 'Example report', summary: 'Example summary.', content: '# Example report\n' }).report;
    const checkpoint = checkpointWorkspaceResearch(options, 'Canonical research checkpoint');
    assert.equal(checkpoint.status, 'committed', checkpoint.error);
    assert.ok(existsSync(join(workspaceRoot, 'memories', `${memory.id}.md`)));
    assert.equal(existsSync(join(workspaceRoot, 'memories', `${foreign.id}.md`)), false);
    assert.equal(checkpointWorkspaceResearch(options, 'Unchanged publication').status, 'unchanged');

    const claimPath = `claims/${claim.id}.json`;
    const exported = JSON.parse(readFileSync(join(workspaceRoot, claimPath), 'utf8'));
    exported.status = 'verified';
    writeFileSync(join(workspaceRoot, claimPath), JSON.stringify(exported));
    assert.throws(() => importWorkspaceResearchFile(options, claimPath, claim.revision), /host-managed/);
    assert.equal(claims.get(claim.id).status, 'hypothesis');
    exported.status = 'hypothesis';
    exported.summary = 'An explicitly revised hypothesis.';
    writeFileSync(join(workspaceRoot, claimPath), JSON.stringify(exported));
    importWorkspaceResearchFile(options, claimPath, claim.revision);
    assert.equal(claims.get(claim.id).summary, exported.summary);
    assert.equal(checkpointWorkspaceResearch(options, 'Imported claim').status, 'committed');

    const memoryPath = `memories/${memory.id}.md`;
    writeFileSync(join(workspaceRoot, memoryPath), readFileSync(join(workspaceRoot, memoryPath), 'utf8').replace('Original explanation.', 'Revised explanation.'));
    importWorkspaceResearchFile(options, memoryPath, memory.revision);
    assert.equal(graph.get(memory.id).body, 'Revised explanation.');
    assert.equal(checkpointWorkspaceResearch(options, 'Imported memory').status, 'committed');

    const reportPath = `reports/${report.id}/report.md`;
    writeFileSync(join(workspaceRoot, reportPath), '# Revised example report\n');
    importWorkspaceResearchFile(options, reportPath, report.revision);
    assert.match(reports.get(report.id).content, /Revised/);
    assert.equal(checkpointWorkspaceResearch(options, 'Imported report').status, 'committed');

    const runbookPath = `runbooks/${runbook.id}/runbook.ipynb`;
    const notebook = JSON.parse(readFileSync(join(workspaceRoot, runbookPath), 'utf8'));
    notebook.cells[1].source = ['echo revised-example'];
    writeFileSync(join(workspaceRoot, runbookPath), JSON.stringify(notebook));
    importWorkspaceResearchFile(options, runbookPath, runbook.revision);
    assert.equal(runbooks.get(runbook.id).contentRevision, runbook.contentRevision + 1);
    assert.equal(checkpointWorkspaceResearch(options, 'Imported procedure').status, 'committed');
  } finally {
    reports.close(); runbooks.close(); claims.close(); graph.close(); other.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
