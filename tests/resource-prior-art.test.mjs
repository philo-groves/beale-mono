import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { ResearchResourceCatalog, createPriorArtSearchTool, createPriorArtFetchTool, createResearchResourceTool, projectModelToolResult } from '../packages/research-agent/dist/index.js';

const resource = { id: 'asset-example', kind: 'repository', direction: 'in_scope', locator: 'https://github.com/example-org/example-repository', source: 'explicit_scope' };
const action = (toolName, input, id = 'action-example') => ({ id, toolName, actionClass: 'inspect', input });
const noMatches = { query: 'example', aliases: [], fetchedAt: '2026-09-01T00:00:00.000Z', sources: [{ source: 'nvd', query: 'example', status: 'complete', resultCount: 0 }], records: [], resultCount: 0, returnedSoFar: 0, nextCursor: null, complete: true, disposition: 'no_matches_found', caveat: 'Limited to requested sources.' };
async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'beale-prior-art-'));
  const databasePath = join(directory, 'memory.sqlite');
  const catalogs = [];
  const open = (workspaceId = 'workspace-example', resources = [resource]) => {
    const catalog = new ResearchResourceCatalog({ databasePath, workspaceId, explicitResources: resources });
    catalogs.push(catalog);
    return catalog;
  };
  t.after(async () => { for (const catalog of catalogs.reverse()) catalog.close(); await rm(directory, { recursive: true, force: true }); });
  return { open };
}

test('saved prior art survives scope revisions and remains isolated by workspace and resource', async (t) => {
  const { open } = await fixture(t);
  const catalog = open();
  const id = catalog.list()[0].id;
  const historyId = catalog.priorArt.save(id, 'action-example', noMatches, { sessionId: 'session-example', revision: 'build-example-001' });
  assert.equal(catalog.priorArt.save(id, 'action-example', { ...noMatches, query: 'changed' }), historyId);
  const reopened = open('workspace-example', [{ ...resource, id: 'asset-next-version', name: 'Renamed example', direction: 'out_of_scope' }]);
  assert.equal(reopened.list()[0].id, id);
  assert.equal(reopened.priorArt.requireResource('asset-next-version'), id);
  const detail = reopened.priorArt.get([id], historyId);
  assert.equal(detail.data.query, 'example');
  assert.equal(detail.sessionId, 'session-example');
  assert.equal(detail.revision, 'build-example-001');
  const other = open('workspace-other');
  assert.equal(other.priorArt.list([id]).entries.length, 0);
  assert.throws(() => other.priorArt.get([id], historyId), /not found/);
  assert.throws(() => other.priorArt.save(id, 'action-other', noMatches), /not found/);
  assert.throws(() => reopened.priorArt.get(['resource-other'], historyId), /not found/);
  const removed = open('workspace-example', []);
  assert.equal(removed.priorArt.list([id]).entries.length, 1);
});

test('history pagination is stable when newer searches arrive and rejects malformed cursors', async (t) => {
  const { open } = await fixture(t);
  const catalog = open();
  const id = catalog.list()[0].id;
  for (let index = 0; index < 4; index++) catalog.priorArt.save(id, `action-${index}`, noMatches);
  const first = catalog.priorArt.list([id], undefined, 2);
  catalog.priorArt.save(id, 'action-newer', noMatches);
  const second = catalog.priorArt.list([id], first.nextBefore, 2);
  assert.equal(new Set([...first.entries, ...second.entries].map((entry) => entry.id)).size, 4);
  assert.equal(second.nextBefore, null);
  assert.throws(() => catalog.priorArt.list([id], -1), /bounds/);
  assert.throws(() => catalog.priorArt.list([id], undefined, 100), /bounds/);
});

test('resource-linked search persists source failures and requires a valid resource before network access', async (t) => {
  const { open } = await fixture(t);
  const catalog = open();
  const id = catalog.list()[0].id;
  let fetches = 0;
  const tool = createPriorArtSearchTool({ history: { store: catalog.priorArt, sessionId: 'session-example' }, fetch: async () => { fetches++; return new Response('Unavailable', { status: 503 }); } });
  assert.ok(tool.parameters.required.includes('resourceId'));
  assert.ok(tool.descriptor.inputSchema.required.includes('resourceId'));
  assert.equal((await tool.execute(action('prior_art.search', { query: 'example' }))).status, 'error');
  assert.equal(fetches, 0);
  const result = await tool.execute(action('prior_art.search', { query: 'example', resourceId: id }));
  assert.equal(result.status, 'error');
  const saved = catalog.priorArt.get([id], result.output.savedHistoryId);
  assert.equal(saved.data.complete, false);
  assert.equal(saved.data.disposition, 'sources_unavailable');
  assert.equal(saved.data.sources[0].status, 'error');
  assert.equal(saved.data.request.query, 'example');
});

test('document archival retains full extracted text and links for offline, paged recall', async (t) => {
  const { open } = await fixture(t);
  const catalog = open();
  const id = catalog.list()[0].id;
  let fetches = 0;
  const tool = createPriorArtFetchTool({ history: { store: catalog.priorArt }, fetch: async () => {
    fetches++;
    return new Response(`<title>Example bulletin</title><p>${'a'.repeat(18000)}</p>${Array.from({ length: 45 }, (_, i) => `<a href="https://example.test/${i}">Source ${i}</a>`).join('')}`, { headers: { 'content-type': 'text/html', etag: 'example-etag' } });
  } });
  const result = await tool.execute(action('prior_art.fetch', { resourceId: id, url: 'https://example.test/bulletin', maxCharacters: 100 }));
  assert.equal(result.status, 'complete');
  assert.equal(result.output.text.length, 100);
  const first = catalog.priorArt.get([id], result.output.savedHistoryId);
  const second = catalog.priorArt.get([id], result.output.savedHistoryId, first.nextOffset);
  const third = catalog.priorArt.get([id], result.output.savedHistoryId, second.nextOffset);
  assert.equal(first.data.text.length, 8000);
  assert.equal(first.data.contentHash, result.output.contentHash);
  assert.equal(first.data.etag, 'example-etag');
  assert.equal(first.data.links.length + second.data.links.length + third.data.links.length, 45);
  assert.equal(third.nextOffset, null);
  const reader = createResearchResourceTool({ catalog, authorizationRecorded: false, authorizeScopeRelevance: async () => { throw new Error('No review expected.'); } });
  const recall = await reader.execute(action('resource.catalog', { operation: 'history', resourceId: id, historyId: result.output.savedHistoryId }));
  assert.equal(recall.output.data.contentHash, first.data.contentHash);
  assert.equal(fetches, 1);
});

test('complete advisory recall preserves large source fields beyond compact model cards', async (t) => {
  const { open } = await fixture(t);
  const catalog = open();
  const id = catalog.list()[0].id;
  const record = { id: 'EXAMPLE-001', aliases: [], source: 'osv', kind: 'advisory', summary: 'Example record', published: null, modified: null, affected: [], affectedDetails: [], references: [], url: 'https://example.test/advisory', detailUrl: 'https://example.test/advisory', sourceData: { details: '"\\\n'.repeat(6000), withdrawn: '2026-08-01' } };
  const historyId = catalog.priorArt.save(id, 'action-large', { ...noMatches, records: [record], resultCount: 1, disposition: 'matches_found' });
  const reader = createResearchResourceTool({ catalog, authorizationRecorded: false, authorizeScopeRelevance: async () => { throw new Error('No review expected.'); } });
  let text = '', offset = 0;
  do {
    const result = await reader.execute(action('resource.catalog', { operation: 'history', resourceId: id, historyId, recordIndex: 0, textOffset: offset }));
    assert.ok(result.output.text.length <= 8000);
    assert.doesNotMatch(JSON.stringify(projectModelToolResult(result)), /Tool result truncated/);
    text += result.output.text;
    offset = result.output.nextTextOffset;
  } while (offset !== null);
  assert.deepEqual(JSON.parse(text), record);
  assert.throws(() => catalog.priorArt.readRecord([id], historyId, 5), /not found/);
});
