import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { WorkspaceService } from '../src/main/workspaceService';
import { researchKitDefinition, researchKitResourceKey } from '../src/shared/researchKits';
import { resolvedTestResearchProfile } from './researchProfileFixture';

const createdDirectories: string[] = [];

afterEach(() => {
  for (const directory of createdDirectories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function workspaceService(root: string, githubFetch?: typeof fetch): WorkspaceService {
  return new WorkspaceService(() => undefined, {
    workspaceRegistryDirectory: join(root, 'registry'),
    appServerDatabasePath: join(root, 'memory.sqlite'),
    appServerArtifactDirectory: join(root, 'artifacts'),
    researchProfileResolver: () => resolvedTestResearchProfile(),
    githubFetch
  });
}

function workspaceDirectory(prefix: string): { root: string; workspacePath: string } {
  const root = mkdtempSync(join(tmpdir(), prefix));
  createdDirectories.push(root);
  const workspacePath = join(root, 'workspace');
  mkdirSync(workspacePath, { recursive: true });
  return { root, workspacePath };
}

describe('Research Kit refresh', () => {
  it('refreshes selected Apple repository metadata while preserving clones and manual resources', async () => {
    const { root, workspacePath } = workspaceDirectory('beale-apple-kit-refresh-');
    const service = workspaceService(root, async () => new Response(JSON.stringify([{
      name: 'swift',
      html_url: 'https://github.com/apple-oss-distributions/swift',
      archived: true
    }]), { status: 200, headers: { 'content-type': 'application/json' } }));
    try {
      service.createScopedWorkspace({
        workspacePath,
        researchKitId: 'apple-security-bounty',
        workspaceName: 'Apple Research',
        researchSubjectName: 'Apple',
        scopeOwner: 'Apple',
        descriptionMarkdown: 'Old guidance.',
        rules: [],
        expiresAt: null,
        assets: [{
          direction: 'in_scope',
          kind: 'repo',
          value: 'https://github.com/apple-oss-distributions/swift',
          sensitivity: 'public',
          attributes: {
            source: 'apple-oss',
            repositoryUrl: 'https://github.com/apple-oss-distributions/swift',
            displayName: 'Old Swift',
            clonedDirectory: 'C:\\Research\\swift'
          }
        }, {
          direction: 'in_scope',
          kind: 'documentation',
          value: 'https://example.test/manual',
          sensitivity: 'public',
          attributes: { source: 'manual' }
        }]
      });

      const refreshed = await service.refreshResearchKit({});
      expect(refreshed).toMatchObject({
        researchKitId: 'apple-security-bounty',
        resourcesRefreshed: 1,
        rulesRefreshed: researchKitDefinition('apple-security-bounty').onboardingDefaults?.rules.length,
        guidanceRefreshed: true
      });
      expect(refreshed.snapshot.activeScope.descriptionMarkdown).toBe(
        researchKitDefinition('apple-security-bounty').onboardingDefaults?.descriptionMarkdown
      );
      expect(refreshed.snapshot.activeScope.assets).toEqual(expect.arrayContaining([
        expect.objectContaining({
          kind: 'repo',
          value: 'https://github.com/apple-oss-distributions/swift',
          attributes: expect.objectContaining({
            displayName: 'swift',
            archived: true,
            clonedDirectory: 'C:\\Research\\swift',
            researchKitId: 'apple-security-bounty',
            researchKitRefreshedAt: refreshed.refreshedAt
          })
        }),
        expect.objectContaining({ kind: 'documentation', value: 'https://example.test/manual' })
      ]));
    } finally {
      service.close();
    }
  }, 15_000);

  it('manages selected MSRC Windows resources while preserving manual resources and checkout metadata', async () => {
    const { root, workspacePath } = workspaceDirectory('beale-msrc-kit-refresh-');
    const service = workspaceService(root);
    try {
      service.createScopedWorkspace({
        workspacePath,
        researchKitId: 'msrc',
        workspaceName: 'Microsoft Research',
        researchSubjectName: 'Microsoft',
        scopeOwner: 'Microsoft',
        descriptionMarkdown: 'Old guidance.',
        rules: [],
        expiresAt: null,
        assets: [{
          direction: 'in_scope',
          kind: 'documentation',
          value: 'https://example.test/manual',
          sensitivity: 'public',
          attributes: { source: 'manual' }
        }, {
          direction: 'in_scope',
          kind: 'repo',
          value: 'https://github.com/microsoft/terminal',
          sensitivity: 'public',
          attributes: { source: 'msrc-windows', clonedDirectory: 'C:\\ExampleResearch\\terminal' }
        }]
      });
      const definition = researchKitDefinition('msrc');
      const catalog = definition.resourceCatalog?.resources ?? [];
      const consoleSource = catalog.find((asset) => asset.value === 'https://github.com/microsoft/terminal');
      const defenderSandbox = catalog.find((asset) => asset.value === 'MsMpEngCP.exe');
      expect(consoleSource).toBeDefined();
      expect(defenderSandbox).toBeDefined();
      const refreshed = await service.refreshResearchKit({
        selectedResourceKeys: [researchKitResourceKey(consoleSource!), researchKitResourceKey(defenderSandbox!)]
      });
      expect(refreshed).toMatchObject({
        researchKitId: 'msrc',
        resourcesRefreshed: 2,
        rulesRefreshed: definition.onboardingDefaults?.rules.length,
        guidanceRefreshed: true
      });
      expect(refreshed.snapshot.activeScope.assets).toEqual(expect.arrayContaining([
        expect.objectContaining({ value: 'MsMpEngCP.exe', attributes: expect.objectContaining({ researchKitId: 'msrc' }) }),
        expect.objectContaining({ value: 'https://github.com/microsoft/terminal', kind: 'repo', attributes: expect.objectContaining({ clonedDirectory: 'C:\\ExampleResearch\\terminal' }) }),
        expect.objectContaining({ value: 'https://example.test/manual', attributes: { source: 'manual' } })
      ]));
      expect(refreshed.snapshot.workspaceRules.map((rule) => rule.text)).toEqual(definition.onboardingDefaults?.rules);
      expect(refreshed.snapshot.activeScope.descriptionMarkdown).toBe(definition.onboardingDefaults?.descriptionMarkdown);
      const narrowed = await service.refreshResearchKit({ selectedResourceKeys: [researchKitResourceKey(defenderSandbox!)] });
      expect(narrowed.resourcesRefreshed).toBe(1);
      expect(narrowed.snapshot.activeScope.assets.map((asset) => asset.value)).toEqual(expect.arrayContaining(['MsMpEngCP.exe', 'https://example.test/manual']));
      expect(narrowed.snapshot.activeScope.assets.some((asset) => asset.value === 'https://github.com/microsoft/terminal')).toBe(false);
      await expect(service.refreshResearchKit({ selectedResourceKeys: ['unknown'] })).rejects.toThrow('Unknown Research Kit resource');
    } finally {
      service.close();
    }
  });

  it('refreshes imported Meta scope examples, rules, and guidance while preserving manual resources', async () => {
    const { root, workspacePath } = workspaceDirectory('beale-meta-kit-refresh-');
    const service = workspaceService(root);
    try {
      service.createScopedWorkspace({
        workspacePath,
        researchKitId: 'meta-bug-bounty',
        workspaceName: 'ExampleCo Research',
        researchSubjectName: 'ExampleCo',
        scopeOwner: 'ExampleCo',
        descriptionMarkdown: 'Old guidance.',
        rules: [],
        expiresAt: null,
        assets: [{
          direction: 'in_scope',
          kind: 'domain',
          value: 'facebook.com',
          sensitivity: 'public',
          attributes: { source: 'meta-bug-bounty', researchKitId: 'meta-bug-bounty', note: 'Retain this note.' }
        }, {
          direction: 'out_of_scope',
          kind: 'domain',
          value: 'fbsbx.com',
          sensitivity: 'public',
          attributes: { source: 'meta-bug-bounty', researchKitId: 'meta-bug-bounty' }
        }, {
          direction: 'in_scope',
          kind: 'domain',
          value: 'example.test',
          sensitivity: 'public',
          attributes: { source: 'manual' }
        }]
      });

      const refreshed = await service.refreshResearchKit({});
      const definition = researchKitDefinition('meta-bug-bounty');
      expect(refreshed).toMatchObject({
        researchKitId: 'meta-bug-bounty',
        resourcesRefreshed: 2,
        rulesRefreshed: definition.onboardingDefaults?.rules.length,
        guidanceRefreshed: true
      });
      expect(refreshed.snapshot.activeScope.assets).toHaveLength(3);
      expect(refreshed.snapshot.activeScope.assets).toEqual(expect.arrayContaining([
        expect.objectContaining({
          direction: 'in_scope',
          value: 'facebook.com',
          attributes: expect.objectContaining({
            note: 'Retain this note.',
            researchKitSourceUrl: 'https://bugbounty.meta.com/scope/',
            researchKitRefreshedAt: refreshed.refreshedAt
          })
        }),
        expect.objectContaining({ direction: 'out_of_scope', value: 'fbsbx.com' }),
        expect.objectContaining({ value: 'example.test', attributes: { source: 'manual' } })
      ]));
      expect(refreshed.snapshot.workspaceRules.map((rule) => rule.text)).toEqual(definition.onboardingDefaults?.rules);
      expect(refreshed.snapshot.activeScope.descriptionMarkdown).toBe(definition.onboardingDefaults?.descriptionMarkdown);
    } finally {
      service.close();
    }
  });

  it('refreshes selected Google OSS tiers while preserving checkout metadata', async () => {
    const { root, workspacePath } = workspaceDirectory('beale-google-oss-kit-refresh-');
    const service = workspaceService(root);
    try {
      service.createScopedWorkspace({
        workspacePath,
        researchKitId: 'google-oss-vrp',
        workspaceName: 'Google OSS Research',
        researchSubjectName: 'Google Open Source Software',
        scopeOwner: 'Google Open Source Software',
        descriptionMarkdown: 'Old guidance.',
        rules: [],
        expiresAt: null,
        assets: [{
          direction: 'in_scope',
          kind: 'repo',
          value: 'https://github.com/google/gson',
          sensitivity: 'public',
          attributes: {
            source: 'google-oss-vrp',
            repositoryUrl: 'https://github.com/google/gson',
            repositoryTier: 'OT1',
            clonedDirectory: 'C:\\Research\\gson'
          }
        }]
      });

      const refreshed = await service.refreshResearchKit({});
      const definition = researchKitDefinition('google-oss-vrp');
      expect(refreshed).toMatchObject({
        researchKitId: 'google-oss-vrp',
        resourcesRefreshed: 1,
        rulesRefreshed: definition.onboardingDefaults?.rules.length,
        guidanceRefreshed: true
      });
      expect(refreshed.snapshot.activeScope.assets).toEqual(expect.arrayContaining([
        expect.objectContaining({
          value: 'https://github.com/google/gson',
          attributes: expect.objectContaining({
            displayName: 'google/gson',
            repositoryTier: 'OT0',
            clonedDirectory: 'C:\\Research\\gson',
            researchKitId: 'google-oss-vrp',
            researchKitRefreshedAt: refreshed.refreshedAt
          })
        })
      ]));
      expect(refreshed.snapshot.workspaceRules.map((rule) => rule.text)).toEqual(definition.onboardingDefaults?.rules);
      expect(refreshed.snapshot.activeScope.descriptionMarkdown).toBe(definition.onboardingDefaults?.descriptionMarkdown);
    } finally {
      service.close();
    }
  });

  it('does not expose a refresh operation for the General kit', async () => {
    const { root, workspacePath } = workspaceDirectory('beale-general-kit-refresh-');
    const service = workspaceService(root);
    try {
      service.createScopedWorkspace({
        workspacePath,
        researchKitId: 'general',
        workspaceName: 'General Research',
        researchSubjectName: 'General',
        scopeOwner: 'General',
        descriptionMarkdown: '',
        rules: [],
        expiresAt: null,
        assets: []
      });
      await expect(service.refreshResearchKit({})).rejects.toThrow('no imports to refresh');
    } finally {
      service.close();
    }
  });
});
