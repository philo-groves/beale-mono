import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { WorkspaceResourceDialog, WorkspaceResourceEditor } from '../src/renderer/features/workspaces/WorkspaceUnderstandingView';
import { ResourcePriorArtContent } from '../src/renderer/features/workspaces/ResourcePriorArtView';
import type { ScopeAsset, ResourcePriorArtDetail } from '../src/shared/types';

const asset: ScopeAsset = { id: 'asset-example', scopeVersionId: 'scope-example', direction: 'in_scope', kind: 'documentation', value: 'https://example.test/docs', sensitivity: 'public', attributes: { displayName: 'Example documentation' }, createdAt: '2026-09-01T00:00:00.000Z' };
const callbacks = { onClose: () => undefined, onSubmit: async () => undefined };

describe('resource detail views', () => {
  it('uses an inline editor for existing resources while preserving the creation dialog', () => {
    const editor = renderToStaticMarkup(createElement(WorkspaceResourceEditor, { ...callbacks, initialAsset: asset, kind: asset.kind, onRemove: async () => undefined }));
    expect(editor).toContain('Back to resources');
    expect(editor).toContain('Example documentation');
    expect(editor).toContain('Save changes');
    expect(editor).toContain('Remove');
    expect(editor).not.toContain('role="dialog"');
    const creation = renderToStaticMarkup(createElement(WorkspaceResourceDialog, { ...callbacks, initialAsset: null, kind: 'documentation' }));
    expect(creation).toContain('role="dialog"');
    expect(creation).toContain('Add resource');
    expect(creation).not.toContain('Back to resources');
  });

  it('shows coverage failures and pending source pages without implying a clean search', () => {
    const detail: ResourcePriorArtDetail = { id: 'history-example', resourceId: 'resource-example', kind: 'search', title: 'Example query', recordedAt: '2026-09-01T00:00:00.000Z', sessionId: 'session-example', revision: 'build-example-001', disposition: 'incomplete', resultCount: 0, offset: 0, nextOffset: null, data: {
      query: 'example', aliases: [], fetchedAt: '2026-09-01T00:00:00.000Z', sources: [{ source: 'nvd', query: 'example', status: 'error', resultCount: 0, error: 'Source unavailable' }], records: [], resultCount: 0, returnedSoFar: 0, nextCursor: 'example-cursor', complete: false, disposition: 'incomplete', caveat: 'Limited coverage.'
    } };
    const html = renderToStaticMarkup(createElement(ResourcePriorArtContent, { detail }));
    expect(html).toContain('Source coverage incomplete');
    expect(html).toContain('Source unavailable');
    expect(html).toContain('More source results remained');
    expect(html).toContain('build-example-001');
    expect(html).toContain('session-example');
    expect(html).not.toContain('No matches in requested sources');
  });

  it('renders archived source content as inert text and rejects executable source links', () => {
    const detail: ResourcePriorArtDetail = { id: 'history-example', resourceId: 'resource-example', kind: 'document', title: 'Example source', recordedAt: '2026-09-01T00:00:00.000Z', sessionId: null, revision: null, disposition: 'source_saved', resultCount: 1, offset: 0, nextOffset: null, data: {
      title: 'Example source', requestedUrl: 'https://example.test/source', url: 'https://example.test/source', fetchedAt: '2026-09-01T00:00:00.000Z', contentType: 'text/html', contentHash: 'example-hash', etag: 'example-etag', lastModified: null, text: '<script>example()</script>', links: [{ url: 'javascript:example()', text: 'Untrusted link' }, { url: 'https://example.test/reference', text: 'Reference' }]
    } };
    const html = renderToStaticMarkup(createElement(ResourcePriorArtContent, { detail }));
    expect(html).toContain('&lt;script&gt;');
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('href="javascript:');
    expect(html).toContain('href="https://example.test/reference"');
    expect(html).toContain('example-hash');
    expect(html).toContain('example-etag');
  });
});
