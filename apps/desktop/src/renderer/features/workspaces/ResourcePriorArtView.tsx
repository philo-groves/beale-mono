import { useCallback, useEffect, useRef, useState } from 'react';
import type { JSX } from 'react';
import type { ResourcePriorArtDetail, ResourcePriorArtPage } from '@shared/types';
import { errorMessage } from '../../lib/errors';

export function ResourcePriorArtView({ workspaceId, assetIds }: { workspaceId: string; assetIds: string[] }): JSX.Element {
  const [page, setPage] = useState<ResourcePriorArtPage | null>(null);
  const [detail, setDetail] = useState<ResourcePriorArtDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const request = useRef(0);
  const retryRequest = useRef<{ kind: 'list'; before?: number } | { kind: 'detail'; id: string; offset: number }>({ kind: 'list' });
  const identity = JSON.stringify(assetIds);
  const load = useCallback(async (before?: number): Promise<void> => {
    retryRequest.current = { kind: 'list', ...(before !== undefined ? { before } : {}) };
    const version = ++request.current;
    setLoading(true);
    setError(null);
    try {
      const result = await window.beale.listResourcePriorArt(workspaceId, JSON.parse(identity) as string[], before);
      if (version !== request.current) return;
      setPage((current) => before && current ? { ...result, entries: [...current.entries, ...result.entries] } : result);
    } catch (caught) {
      if (version === request.current) setError(errorMessage(caught));
    } finally {
      if (version === request.current) setLoading(false);
    }
  }, [workspaceId, identity]);
  useEffect(() => {
    setPage(null);
    setDetail(null);
    void load();
    return () => { request.current += 1; };
  }, [load]);
  const read = async (id: string, offset = 0): Promise<void> => {
    retryRequest.current = { kind: 'detail', id, offset };
    const version = ++request.current;
    setLoading(true);
    setError(null);
    try {
      const result = await window.beale.getResourcePriorArt(workspaceId, assetIds, id, offset);
      if (version === request.current) setDetail(result);
    } catch (caught) {
      if (version === request.current) setError(errorMessage(caught));
    } finally {
      if (version === request.current) setLoading(false);
    }
  };
  return (
    <section className="resource-prior-art" aria-label="Resource prior art" aria-busy={loading}>
      <header className="resource-detail-header">
        <h2>Prior art</h2>
        <button className="secondary-button" disabled={loading} onClick={() => { setDetail(null); void load(); }} type="button">Refresh saved history</button>
      </header>
      <p>Saved public-history searches and source documents for this resource. Matches require an applicability review; no matches do not establish novelty.</p>
      {loading ? <p role="status">Loading saved history…</p> : null}
      {error ? <div role="alert"><p>{error}</p><button className="secondary-button" disabled={loading} onClick={() => { const retry = retryRequest.current; void (retry.kind === 'detail' ? read(retry.id, retry.offset) : load(retry.before)); }} type="button">Retry</button></div> : null}
      {page?.entries.length === 0 ? <p>No prior art has been saved for this resource. Future resource-linked searches will appear here. Earlier session traces have not been imported.</p> : null}
      {detail ? <>
        <button className="secondary-button" disabled={loading} onClick={() => { setDetail(null); setError(null); }} type="button">Back to saved history</button>
        <ResourcePriorArtContent detail={detail} />
        <div className="resource-detail-actions">
          {detail.offset > 0 ? <button className="secondary-button" disabled={loading} onClick={() => void read(detail.id, Math.max(0, detail.offset - (detail.kind === 'search' ? 5 : 1)))} type="button">Previous page</button> : null}
          {detail.nextOffset !== null ? <button className="secondary-button" disabled={loading} onClick={() => void read(detail.id, detail.nextOffset!)} type="button">Next page</button> : null}
        </div>
      </> : <>
        <ul className="resource-history-list">
          {page?.entries.map((entry) => <li key={entry.id}>
            <button disabled={loading} onClick={() => void read(entry.id)} type="button">
              <strong>{entry.title}</strong>
              <span>{historyDisposition(entry.disposition)} · {entry.kind === 'search' ? `${entry.resultCount} ${entry.resultCount === 1 ? 'record' : 'records'} on this search page` : 'Archived document'}</span>
              <small>{entry.recordedAt}{entry.revision ? ` · Revision ${entry.revision}` : ''}</small>
            </button>
          </li>)}
        </ul>
        {page?.nextBefore ? <button className="secondary-button" disabled={loading} onClick={() => void load(page.nextBefore!)} type="button">Load older history</button> : null}
      </>}
    </section>
  );
}

export function ResourcePriorArtContent({ detail }: { detail: ResourcePriorArtDetail }): JSX.Element {
  return <article className="resource-history-detail">
    <h3>{detail.title}</h3>
    <p>{historyDisposition(detail.disposition)} · Retrieved {detail.recordedAt}</p>
    {detail.revision ? <p>Resource revision: {detail.revision}</p> : null}
    {detail.sessionId ? <p>Recorded in session: {detail.sessionId}</p> : null}
    {detail.kind === 'search' ? <>
      <p>{detail.data.complete ? 'Requested source coverage complete.' : 'Source coverage incomplete.'} {detail.data.nextCursor ? 'More source results remained when this page was saved.' : ''}</p>
      <ul>{detail.data.sources.map((source, index) => <li key={index}>
        <strong>{source.source}</strong>: {source.status} · {source.query}
        {source.error ? <p>{source.error}</p> : null}{source.limitation ? <p>{source.limitation}</p> : null}
        {source.url ? <SourceLink url={source.url} /> : null}
      </li>)}</ul>
      {detail.data.records.map((record) => <article className="resource-history-record" key={`${record.source}:${record.id}`}>
        <h4>{record.id} · {record.source}</h4>
        <p>{record.summary}</p>
        {record.aliases.length ? <p>Aliases: {record.aliases.join(', ')}</p> : null}
        <p>Published: {record.published ?? 'Unknown'} · Modified: {record.modified ?? 'Unknown'}</p>
        <SourceLink url={record.url} />
        <details><summary>Affected versions and source data</summary><pre>{JSON.stringify({ affected: record.affected, affectedDetails: record.affectedDetails, sourceData: record.sourceData }, null, 2)}</pre></details>
        {record.references.length ? <details><summary>References ({record.references.length})</summary><ul>{record.references.map((url, index) => <li key={index}><SourceLink url={url} /></li>)}</ul></details> : null}
      </article>)}
      <details><summary>Saved search parameters and continuation</summary><pre>{JSON.stringify({ request: detail.data.request, nextCursor: detail.data.nextCursor }, null, 2)}</pre></details>
    </> : <>
      <SourceLink url={detail.data.url} />
      <p>Content hash: <code>{detail.data.contentHash}</code></p>
      <p>Source last modified: {detail.data.lastModified ?? 'Not supplied'} · ETag: {detail.data.etag ?? 'Not supplied'}</p>
      <pre>{detail.data.text}</pre>
      <ul>{detail.data.links.map((link, index) => <li key={index}><SourceLink url={link.url} label={link.text} /></li>)}</ul>
    </>}
  </article>;
}

function SourceLink({ url, label }: { url: string; label?: string }): JSX.Element {
  const [error, setError] = useState<string | null>(null);
  let allowed = false;
  try { const parsed = new URL(url); allowed = ['http:', 'https:'].includes(parsed.protocol) && !parsed.username && !parsed.password; } catch { /* Render invalid sources as plain text. */ }
  return <>{allowed ? <a href={url} onClick={(event) => {
    event.preventDefault();
    void window.beale.openExternalUrl(url).catch((caught: unknown) => setError(errorMessage(caught)));
  }}>{label || url}</a> : <span>{label || url}</span>}{error ? <span role="alert">{error}</span> : null}</>;
}

function historyDisposition(value: string): string {
  return ({ matches_found: 'Matches found', no_matches_found: 'No matches in requested sources', incomplete: 'Incomplete search', sources_unavailable: 'Sources unavailable', source_saved: 'Source saved' } as Record<string, string>)[value] ?? value;
}
