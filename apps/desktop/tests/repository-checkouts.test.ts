import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { sourceRepositoryCheckoutPath } from '@beale/research-agent';
import { describe, expect, it } from 'vitest';
import { repositoryCheckoutsForScope } from '../src/main/workspaceService';
import type { ScopeAsset } from '../src/shared/types';

describe('repository checkout projection', () => {
  it('finds managed checkouts without resource metadata and ignores missing recorded paths', () => {
    const root = mkdtempSync(join(tmpdir(), 'beale-checkouts-'));
    const store = join(root, 'repositories');
    const url = 'https://github.com/example-org/example-repository';
    const checkout = sourceRepositoryCheckoutPath(url, 'example-ref', store);
    const asset: ScopeAsset = {
      id: 'asset-example',
      scopeVersionId: 'scope-example',
      direction: 'in_scope',
      kind: 'repo',
      value: url,
      sensitivity: 'public',
      attributes: { clonedDirectory: join(root, 'missing'), repositoryUrl: url },
      createdAt: '2026-01-01T00:00:00.000Z'
    };
    try {
      expect(repositoryCheckoutsForScope([asset], store)).toEqual({});
      mkdirSync(join(checkout, '.git'), { recursive: true });
      expect(repositoryCheckoutsForScope([asset], store)).toEqual({ [asset.id]: checkout });
      rmSync(join(checkout, '.git'), { recursive: true });
      expect(repositoryCheckoutsForScope([asset], store)).toEqual({});
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
