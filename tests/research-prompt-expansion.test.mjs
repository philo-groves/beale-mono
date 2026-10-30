import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { composeResearchPromptWithContext, expandStoredResearchPrompt } from '../packages/research-agent/dist/index.js';

test('context expansion preserves the researcher objective without model paraphrase', () => {
  const original = 'Test whether attacker-controlled request headers can cross the tenant boundary and expose private data.';
  const context = 'Inspect the parsing boundary and seek evidence of unauthorized disclosure.';
  assert.equal(composeResearchPromptWithContext(original, context),
    `## Researcher objective\n\n${original}\n\n## Added context\n\n${context}`);
  const refined = composeResearchPromptWithContext(composeResearchPromptWithContext(original, context),
    'Prioritize the direct unauthorized disclosure proof.');
  assert.equal(refined.match(/## Researcher objective/g)?.length, 1);
  assert.ok(refined.includes(original));
  assert.doesNotMatch(refined, /Inspect the parsing boundary/);
});

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
    assert.match(result.promptMarkdown, /## Researcher objective\n\nInspect request parsing\./);
    assert.match(result.promptMarkdown, /## Added context/);
    assert.match(result.promptMarkdown, /conflicting framing interpretations/);
    assert.equal(completions.length, 1);
    assert.equal(completions[0].provider, 'xai');
    assert.equal(completions[0].model, 'grok-4.6');
    assert.equal(completions[0].effort, 'low');
    assert.match(completions[0].systemPrompt, /do not rewrite, soften, summarize, or repeat it/);
    assert.doesNotMatch(completions[0].systemPrompt, /selected suggestion lane|\b(?:Discovery|Chaining|Reporting|Longshot)\b/);
    assert.equal(completions[0].authenticationPreferences.xai, 'api_key');
    const payload = JSON.parse(completions[0].prompt);
    assert.equal(payload.originalPrompt, 'Inspect request parsing.');
    assert.equal(payload.researchProfile.lane, undefined);
    assert.equal(payload.workspace.id, 'workspace-test');
  } finally {
    rmSync(workspaceRoot, { recursive: true, force: true });
  }
});

test('an explicit suggestion category stays out of added research context', async () => {
  const workspaceRoot = mkdtempSync(join(tmpdir(), 'app-server-prompt-expansion-lane-'));
  try {
    let request;
    const result = await expandStoredResearchPrompt({
      workspaceId: 'workspace-test', workspaceRoot, databasePath: ':memory:',
      artifactDirectoryPath: join(workspaceRoot, 'artifacts'), researchProfileId: 'security-research',
      memoryEnabled: false, phase: 'longshot', promptMarkdown: 'Inspect an authorized trust boundary.',
      provider: { id: 'xai', model: 'grok-4.6' },
    }, {
      completeText: async (options) => {
        request = options;
        return { text: JSON.stringify({ promptMarkdown: 'The synthetic service accepts requests from principals with different access rights. Identify the input each principal controls, the authority decision that consumes it, and a comparison that would show whether protected data crosses that boundary. Preserve evidence of an ordinary authorized response as a control, and state the observed effect before estimating its broader impact.' }) };
      },
    });
    assert.equal(result.phase, 'longshot');
    assert.doesNotMatch(request.systemPrompt, /\bLongshot\b|selected suggestion lane/);
    assert.doesNotMatch(request.prompt, /\bLongshot\b|"lane"|"workflowId"/);
    assert.doesNotMatch(result.promptMarkdown, /\bLongshot\b/);
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

test('research prompt expansion adds context to a long objective without requiring a longer addendum', async () => {
  const workspaceRoot = mkdtempSync(join(tmpdir(), 'app-server-prompt-expansion-long-'));
  const original = 'Investigate the synthetic cross-tenant trust boundary. '.repeat(120).trim();
  try {
    const result = await expandStoredResearchPrompt({
      workspaceId: 'workspace-test',
      workspaceRoot,
      databasePath: ':memory:',
      artifactDirectoryPath: join(workspaceRoot, 'artifacts'),
      researchProfileId: 'security-research',
      memoryEnabled: false,
      promptMarkdown: original,
      provider: { id: 'xai', model: 'grok-4.6' },
    }, {
      completeText: async () => ({
        text: JSON.stringify({ promptMarkdown: 'Examine attacker control at the trust boundary and seek direct evidence of unauthorized cross-tenant data access.' }),
      }),
    });
    assert.ok(result.promptMarkdown.includes(original));
    assert.match(result.promptMarkdown, /unauthorized cross-tenant data access/);
  } finally {
    rmSync(workspaceRoot, { recursive: true, force: true });
  }
});
