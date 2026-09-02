import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { expandStoredResearchPrompt } from '../packages/research-agent/dist/index.js';

test('research prompt expansion selects the profile default lane and validates bounded JSON output', async () => {
  const workspaceRoot = mkdtempSync(join(tmpdir(), 'app-server-prompt-expansion-'));
  const completions = [];
  try {
    const result = await expandStoredResearchPrompt({
      workspaceId: 'workspace-test',
      workspaceRoot,
      databasePath: ':memory:',
      artifactDirectoryPath: join(workspaceRoot, 'artifacts'),
      researchProfileId: 'security-research',
      memoryEnabled: false,
      promptMarkdown: 'Inspect request parsing.',
      provider: {
        id: 'xai',
        model: 'grok-4.6',
        reasoningEffort: 'low',
        authenticationPreferences: { xai: 'api_key' },
      },
    }, {
      completeText: async (options) => {
        completions.push(options);
        return {
          text: '```json\n{"promptMarkdown":"Inspect the recorded in-scope service request parser for conflicting framing interpretations, preserving contrary evidence and identifying the exact consumer boundary needed to establish impact."}\n```',
        };
      },
    });

    assert.equal(result.phase, 'discovery');
    assert.match(result.promptMarkdown, /conflicting framing interpretations/);
    assert.equal(completions.length, 1);
    assert.equal(completions[0].provider, 'xai');
    assert.equal(completions[0].model, 'grok-4.6');
    assert.equal(completions[0].effort, 'low');
    assert.equal(completions[0].authenticationPreferences.xai, 'api_key');
    const payload = JSON.parse(completions[0].prompt);
    assert.equal(payload.originalPrompt, 'Inspect request parsing.');
    assert.equal(payload.researchProfile.lane.id, 'discovery');
    assert.equal(payload.workspace.id, 'workspace-test');
  } finally {
    rmSync(workspaceRoot, { recursive: true, force: true });
  }
});

test('research prompt expansion rejects non-JSON model output', async () => {
  const workspaceRoot = mkdtempSync(join(tmpdir(), 'app-server-prompt-expansion-invalid-'));
  try {
    await assert.rejects(
      expandStoredResearchPrompt({
        workspaceId: 'workspace-test',
        workspaceRoot,
        databasePath: ':memory:',
        artifactDirectoryPath: join(workspaceRoot, 'artifacts'),
        researchProfileId: 'security-research',
        memoryEnabled: false,
        promptMarkdown: 'Inspect request parsing.',
        provider: { id: 'xai', model: 'grok-4.6' },
      }, {
        completeText: async () => ({ text: 'Inspect everything.' }),
      }),
      /invalid JSON/,
    );
  } finally {
    rmSync(workspaceRoot, { recursive: true, force: true });
  }
});
