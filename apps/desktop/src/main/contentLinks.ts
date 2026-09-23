import { statSync } from 'node:fs';
import { isAbsolute, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export type ContentLinkTarget = { kind: 'web'; url: string } | { kind: 'file'; path: string; directory: boolean };

export function resolveContentLink(value: string, workspacePath: string | null): ContentLinkTarget {
  const link = value.trim();
  if (!link || /[\u0000-\u001f\u007f]/.test(link)) throw new Error('Invalid link.');

  if (/^https?:\/\//i.test(link)) {
    const url = new URL(link);
    if (url.protocol !== 'http:' && url.protocol !== 'https:') throw new Error('Unsupported web link.');
    if (workspacePath && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)) {
      const candidate = resolve(workspacePath, `.${decodeURIComponent(url.pathname)}`);
      const withinWorkspace = relative(workspacePath, candidate);
      if (withinWorkspace && !withinWorkspace.startsWith('..') && !isAbsolute(withinWorkspace)) {
        try {
          if (statSync(candidate).isFile()) return { kind: 'file', path: candidate, directory: false };
        } catch {
          // A local web route need not correspond to a workspace file.
        }
      }
    }
    return { kind: 'web', url: url.href };
  }

  let path: string;
  if (/^file:/i.test(link)) {
    const url = new URL(link);
    if (url.protocol !== 'file:') throw new Error('Unsupported file link.');
    path = fileURLToPath(url);
  } else {
    if (/^[a-z][a-z\d+.-]*:/i.test(link) || link.startsWith('//')) throw new Error('Unsupported link.');
    const encodedPath = link.split(/[?#]/, 1)[0] ?? '';
    const localPath = decodeURIComponent(encodedPath);
    if (!localPath) throw new Error('Invalid file link.');
    if (!isAbsolute(localPath) && !workspacePath) throw new Error('Open a workspace to reveal a relative file link.');
    path = isAbsolute(localPath) ? localPath : resolve(workspacePath!, localPath);
  }

  const file = statSync(path);
  if (!file.isFile() && !file.isDirectory()) throw new Error('The link does not point to a file or directory.');
  return { kind: 'file', path, directory: file.isDirectory() };
}
