import assert from 'node:assert/strict';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { ResearchResourceCatalog } from '../../packages/research-agent/dist/index.js';
import { invokeAppServerProtocol } from '../dist/appServerProtocolClient.js';

test('canonical resource history reads preserve workspace and resource boundaries', async () => {
  const root = await mkdtemp(join(tmpdir(), 'beale-history-protocol-'));
  const storage = { databasePath: join(root, 'memory.sqlite'), artifactDirectoryPath: join(root, 'artifacts') };
  const workspaceId = 'workspace-example';
  const resource = { id: 'asset-example', kind: 'documentation', locator: 'https://example.test/bulletin', direction: 'in_scope', source: 'explicit_scope' };
  const catalog = new ResearchResourceCatalog({ databasePath: storage.databasePath, workspaceId, explicitResources: [resource] });
  try {
    const id = catalog.priorArt.save(catalog.list()[0].id, 'action-example', { requestedUrl: resource.locator, url: resource.locator, title: 'Example bulletin', fetchedAt: '2026-09-01T00:00:00.000Z', contentType: 'text/plain', contentHash: 'example-hash', etag: null, lastModified: null, text: 'Example public history.', links: [] });
    const input = { workspaceId, resources: [{ kind: resource.kind, locator: resource.locator }] };
    const list = await invokeAppServerProtocol('resource.prior_art.list', { args: [], storage, input });
    assert.equal(list.entries[0].id, id);
    const detail = await invokeAppServerProtocol('resource.prior_art.get', { args: [], storage, input: { ...input, id } });
    assert.equal(detail.data.text, 'Example public history.');
    await assert.rejects(invokeAppServerProtocol('resource.prior_art.get', { args: [], storage, input: { ...input, workspaceId: 'workspace-other', id } }), /not found/);
    await assert.rejects(invokeAppServerProtocol('resource.prior_art.get', { args: [], storage, input: { ...input, resources: [{ kind: 'documentation', locator: 'https://example.test/other' }], id } }), /not found/);
    await assert.rejects(invokeAppServerProtocol('resource.prior_art.list', { args: [], storage, input: { ...input, before: 'invalid' } }), /bounds/);
  } finally { catalog.close(); await rm(root, { recursive: true, force: true }); }
});
