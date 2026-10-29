import type { JSX } from 'react';
import { Binary, BookOpen, GitBranch, Globe2, Layers3, Server } from 'lucide-react';
import type { ScopeAssetKind } from '@shared/types';

export const WORKSPACE_ASSET_KINDS: ScopeAssetKind[] = [
  'repo', 'documentation', 'binary', 'service', 'domain', 'other'
];

export function workspaceAssetKindLabel(kind: ScopeAssetKind): string {
  if (kind === 'repo') return 'Repository';
  return `${kind.slice(0, 1).toUpperCase()}${kind.slice(1)}`;
}

export function WorkspaceAssetIcon({ kind, size = 16 }: { kind: ScopeAssetKind; size?: number }): JSX.Element {
  if (kind === 'repo') return <GitBranch size={size} />;
  if (kind === 'documentation') return <BookOpen size={size} />;
  if (kind === 'binary') return <Binary size={size} />;
  if (kind === 'service') return <Server size={size} />;
  if (kind === 'domain') return <Globe2 size={size} />;
  return <Layers3 size={size} />;
}

export function surfaceAssetKind(kind: string | undefined): ScopeAssetKind | null {
  if (!kind) return null;
  if (kind === 'repository' || kind === 'repo') return 'repo';
  if (kind === 'tool') return 'other';
  return WORKSPACE_ASSET_KINDS.find((candidate) => candidate === kind) ?? null;
}
