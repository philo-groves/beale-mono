import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawnSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import {
  initializeWorkspaceProject, checkpointWorkspaceResearch, importWorkspaceResearchFile,
  releaseWorkspaceResearchIndex, rebuildWorkspaceResearchIndex,
  MemoryGraphStore, FindingStore, RunbookStore, ReportStore, CampaignTrackStore,
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
    const tracks = new CampaignTrackStore({ databasePath, context });
    try {
      const track = tracks.ensureForSession({ sessionId: 'session-example', objective: 'Example investigation', source: 'runtime' });
      const attributed = checkpointWorkspaceResearch({ ...options, sessionId: 'session-example' }, 'Linked investigation checkpoint');
      assert.equal(attributed.status, 'committed', attributed.error);
      const investigation = JSON.parse(readFileSync(join(workspaceRoot, 'investigations', track.id, 'record.json'), 'utf8'));
      assert.equal(investigation.schemaVersion, 2);
      assert.deepEqual(investigation.sessions.map((entry) => entry.session_id), ['session-example']);
      for (const field of ['resources', 'questions', 'experiments', 'observations', 'nextActions', 'memoryClaimReviews', 'researchClaimReviews']) {
        assert.ok(Array.isArray(investigation[field]), field);
      }
      assert.ok(existsSync(join(workspaceRoot, 'references', 'campaign-state.json')));
      const message = spawnSync('git', ['log', '-1', '--format=%B'], { cwd: workspaceRoot, encoding: 'utf8', windowsHide: true }).stdout.trim();
      assert.equal(message, `Linked investigation checkpoint\n\nInvestigation-ID: ${track.id}\nSession-ID: session-example`);

      const released = releaseWorkspaceResearchIndex(options);
      assert.equal(released.state, 'released');
      const releasedDatabase = new DatabaseSync(databasePath, { readOnly: true });
      for (const table of ['memory_node_workspaces', 'app_server_research_claims', 'app_server_runbooks', 'app_server_reports', 'campaign_tracks']) {
        const clause = table === 'memory_node_workspaces' ? 'workspace_id=?' : 'workspace_id=?';
        assert.equal(releasedDatabase.prepare(`SELECT COUNT(*) AS count FROM ${table} WHERE ${clause}`).get(options.workspaceId).count, 0, table);
      }
      releasedDatabase.close();
      const rebuilt = rebuildWorkspaceResearchIndex(options);
      assert.equal(rebuilt.state, 'ready');
      assert.equal(graph.get(memory.id).body, 'Revised explanation.');
      assert.equal(claims.get(claim.id).summary, exported.summary);
      assert.match(reports.get(report.id).content, /Revised/);
      assert.equal(runbooks.get(runbook.id).contentRevision, runbook.contentRevision + 1);
      assert.equal(tracks.get(track.id).id, track.id);
    } finally { tracks.close(); }
  } finally {
    reports.close(); runbooks.close(); claims.close(); graph.close(); other.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
