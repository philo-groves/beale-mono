import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { describe, expect, it } from 'vitest';
import { resolveContentLink } from '../src/main/contentLinks';

describe('session content links', () => {
  it('opens HTTP and HTTPS pages in the default browser, including local web servers', () => {
    expect(resolveContentLink('http://localhost:5173/investigations/example/HANDOFF.md', null)).toEqual({
      kind: 'web', url: 'http://localhost:5173/investigations/example/HANDOFF.md'
    });
    expect(resolveContentLink('https://example.test/research', null)).toEqual({
      kind: 'web', url: 'https://example.test/research'
    });
  });

  it('reveals relative, absolute, and file URL paths, and opens directories in the file explorer', () => {
    const workspace = mkdtempSync(join(tmpdir(), 'beale-content-link-'));
    const directory = join(workspace, 'investigations', 'example');
    mkdirSync(directory, { recursive: true });
    const file = join(directory, 'HANDOFF.md');
    writeFileSync(file, 'Synthetic handoff.');

    const expectedFile = { kind: 'file', path: file, directory: false };
    expect(resolveContentLink('investigations/example/HANDOFF.md', workspace)).toEqual(expectedFile);
    expect(resolveContentLink('investigations/example/HANDOFF.md#provenance', workspace)).toEqual(expectedFile);
    expect(resolveContentLink(file, workspace)).toEqual(expectedFile);
    expect(resolveContentLink(pathToFileURL(file).href, null)).toEqual(expectedFile);
    expect(resolveContentLink('http://localhost:5173/investigations/example/HANDOFF.md', workspace)).toEqual(expectedFile);
    expect(resolveContentLink('http://localhost:5173/app', workspace)).toEqual({
      kind: 'web', url: 'http://localhost:5173/app'
    });
    expect(resolveContentLink('investigations/example', workspace)).toEqual({
      kind: 'file', path: directory, directory: true
    });
  });

  it('rejects unsafe schemes and unavailable local files', () => {
    expect(() => resolveContentLink('javascript:alert(1)', null)).toThrow('Unsupported link.');
    expect(() => resolveContentLink('ftp://example.test/file', null)).toThrow('Unsupported link.');
    expect(() => resolveContentLink('missing.md', null)).toThrow('Open a workspace');
    expect(() => resolveContentLink('/no-such-beale-example-file', null)).toThrow();
  });
});
