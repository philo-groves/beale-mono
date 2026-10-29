import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { fetchBugcrowdBrief, normalizeBugcrowdIdentifier } from '../src/main/bugcrowdImport';
import { WorkspaceService } from '../src/main/workspaceService';
import { resolvedTestResearchProfile } from './researchProfileFixture';

const slug = 'example-program';
const sourceUrl = `https://bugcrowd.com/engagements/${slug}`;
const briefUrl = `${sourceUrl}/changelog/11111111-1111-4111-8111-111111111111.json`;
const brief = {
  data: {
    brief: {
      name: 'ExampleCo Security',
      description: '<p>Report privately through Bugcrowd.</p>',
      targetsOverview: '<p>Do not test customer data.</p>',
      additionalInformation: '<p>Use test accounts only.</p>'
    },
    engagement: { code: slug, state: 'in_progress', endsAt: null },
    scope: [{
      name: 'Web targets', inScope: true, targets: [
        { category: 'website', uri: 'https://api.example.test', name: 'Example API' },
        { category: 'other', uri: 'https://github.com/example-org/example-repository', name: 'Example source' },
        { category: 'other', uri: '', name: 'Example mobile application' }
      ]
    }, {
      name: 'Excluded targets', inScope: false, targets: [
        { category: 'website', uri: 'https://private.example.test', name: 'Private portal' }
      ]
    }]
  }
};

const createdDirectories: string[] = [];
afterEach(() => {
  for (const directory of createdDirectories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function mockFetch(): typeof fetch {
  return async (input) => {
    const url = String(input);
    if (url === sourceUrl) return new Response(`<div data-api-endpoints="{&quot;getBriefVersionDocument&quot;:&quot;/engagements/${slug}/changelog/11111111-1111-4111-8111-111111111111&quot;}"></div>`, { status: 200 });
    if (url === briefUrl) return new Response(JSON.stringify(brief), { status: 200, headers: { 'content-type': 'application/json' } });
    throw new Error(`Unexpected URL: ${url}`);
  };
}

describe('Bugcrowd workspace import', () => {
  it('accepts only public Bugcrowd engagement URLs or slugs', () => {
    expect(normalizeBugcrowdIdentifier(sourceUrl)).toBe(slug);
    expect(normalizeBugcrowdIdentifier(slug)).toBe(slug);
    expect(normalizeBugcrowdIdentifier('https://bugcrowd.com.evil.example/engagements/example-program')).toBe('');
    expect(normalizeBugcrowdIdentifier('https://bugcrowd.com/engagements/example-program/other')).toBe('');
    expect(normalizeBugcrowdIdentifier('http://bugcrowd.com/engagements/example-program')).toBe('');
  });

  it('imports grouped public targets while preserving excluded targets and explicit names', async () => {
    const imported = await fetchBugcrowdBrief(sourceUrl, mockFetch());
    expect(imported).toMatchObject({ slug, sourceUrl, name: 'ExampleCo Security' });
    expect(imported.assets).toEqual(expect.arrayContaining([
      expect.objectContaining({ direction: 'in_scope', kind: 'domain', value: 'https://api.example.test' }),
      expect.objectContaining({ direction: 'in_scope', kind: 'repo', value: 'https://github.com/example-org/example-repository' }),
      expect.objectContaining({ direction: 'in_scope', kind: 'other', value: 'Example mobile application' }),
      expect.objectContaining({ direction: 'out_of_scope', value: 'https://private.example.test' })
    ]));
    expect(imported.assets[0]?.attributes).toMatchObject({ researchKitId: 'bugcrowd', bugcrowdSlug: slug, scopeGroup: 'Web targets' });
  });

  it('rejects a brief without an importable in-scope target', async () => {
    const emptyBrief = { ...brief, data: { ...brief.data, scope: [{ name: 'Excluded', inScope: false, targets: [{ name: 'Excluded target' }] }] } };
    const fetcher: typeof fetch = async (input) => String(input) === sourceUrl
      ? new Response(`<div data-api-endpoints="{&quot;getBriefVersionDocument&quot;:&quot;/engagements/${slug}/changelog/11111111-1111-4111-8111-111111111111&quot;}"></div>`)
      : new Response(JSON.stringify(emptyBrief));
    await expect(fetchBugcrowdBrief(slug, fetcher)).rejects.toThrow('in-scope target');
  });

  it('reviews the public brief and refreshes managed resources and rules', async () => {
    const root = mkdtempSync(join(tmpdir(), 'beale-bugcrowd-import-'));
    createdDirectories.push(root);
    const stateFile = join(root, 'app-server.json');
    const previousEnvironment = {
      stateFile: process.env.BEALE_APP_SERVER_STATE_FILE,
      parentPid: process.env.BEALE_APP_SERVER_PARENT_PID,
      port: process.env.BEALE_APP_SERVER_PORT
    };
    process.env.BEALE_APP_SERVER_STATE_FILE = stateFile;
    process.env.BEALE_APP_SERVER_PARENT_PID = String(process.pid);
    process.env.BEALE_APP_SERVER_PORT = '0';
    const workspacePath = join(root, 'workspace');
    mkdirSync(workspacePath, { recursive: true });
    const prompts: string[] = [];
    const service = new WorkspaceService(() => undefined, {
      workspaceRegistryDirectory: join(root, 'registry'),
      appServerDatabasePath: join(root, 'memory.sqlite'),
      appServerArtifactDirectory: join(root, 'artifacts'),
      researchProfileResolver: () => resolvedTestResearchProfile(),
      bugcrowdFetch: mockFetch(),
      providerTextCompletion: async ({ prompt }) => {
        prompts.push(prompt);
        return JSON.stringify({ workspaceName: 'ExampleCo Security', scopeOwner: 'ExampleCo', rules: ['Use test accounts only.', 'Report privately through Bugcrowd.'] });
      }
    });
    try {
      service.setDefaultProviderId('xai');
      service.setProviderModelDefaults('xai', { largeModel: 'grok-4.6', smallModel: 'grok-4.3', reasoningEffort: 'high' });
      const imported = await service.lookupBugcrowdScope(sourceUrl);
      expect(prompts[0]).toContain('"source": "bugcrowd_public_engagement_brief"');
      expect(prompts[0]).toContain('Do not test customer data.');
      expect(imported.rules).toEqual(['Use test accounts only.', 'Report privately through Bugcrowd.']);
      service.createScopedWorkspace({
        workspacePath,
        researchKitId: 'bugcrowd',
        workspaceName: imported.workspaceName,
        researchSubjectName: imported.researchSubjectName,
        scopeOwner: imported.scopeOwner,
        descriptionMarkdown: imported.descriptionMarkdown,
        rules: imported.rules,
        expiresAt: imported.expiresAt,
        assets: imported.assets
      });
      const refreshed = await service.refreshResearchKit({ sourceIdentifier: slug });
      expect(refreshed).toMatchObject({ researchKitId: 'bugcrowd', resourcesRefreshed: 4, rulesRefreshed: 2, guidanceRefreshed: true });
      expect(refreshed.snapshot.activeScope.assets).toEqual(expect.arrayContaining([
        expect.objectContaining({ direction: 'out_of_scope', value: 'https://private.example.test' }),
        expect.objectContaining({ direction: 'in_scope', value: 'https://api.example.test', attributes: expect.objectContaining({ researchKitRefreshedAt: refreshed.refreshedAt }) })
      ]));
      expect(prompts).toHaveLength(2);
    } finally {
      service.close();
      if (existsSync(stateFile)) {
        const record = JSON.parse(readFileSync(stateFile, 'utf8')) as { pid?: unknown };
        if (typeof record.pid === 'number' && record.pid > 0 && record.pid !== process.pid) {
          try { process.kill(record.pid); } catch { /* already stopped */ }
        }
      }
      for (const [key, value] of Object.entries({
        BEALE_APP_SERVER_STATE_FILE: previousEnvironment.stateFile,
        BEALE_APP_SERVER_PARENT_PID: previousEnvironment.parentPid,
        BEALE_APP_SERVER_PORT: previousEnvironment.port
      })) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
    }
  }, 30_000);
});
