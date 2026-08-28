import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import test from 'node:test';
import {
  generateStoredResearchGoalSuggestions,
  selectStoredResearchGoalSuggestion
} from '../packages/research-agent/dist/index.js';

test('app-server goal suggestions generate, persist, cache, and record selection', async () => {
  const root = mkdtempSync(join(tmpdir(), 'honeycrisp-goal-suggestions-'));
  const workspaceRoot = join(root, 'workspace');
  const artifactDirectoryPath = join(root, 'artifacts');
  const databasePath = join(root, 'memory.sqlite');
  mkdirSync(workspaceRoot, { recursive: true });
  mkdirSync(artifactDirectoryPath, { recursive: true });
  initializeSuggestionFixture(databasePath, workspaceRoot);
  const completions = [];
  try {
    const input = {
      workspaceId: 'workspace-test',
      workspaceRoot,
      databasePath,
      artifactDirectoryPath,
      researchProfileId: 'security-research',
      memoryEnabled: false,
      phase: 'discovery',
      provider: {
        id: 'openai-codex',
        smallModel: 'gpt-test',
        reasoningEffort: 'low',
        authenticationPreferences: { 'openai-codex': 'subscription' },
        codexAuthFile: join(root, 'codex-auth.json')
      }
    };
    const dependencies = {
      completeText: async (options) => {
        completions.push(options);
        return {
          text: JSON.stringify({
            candidates: [
              candidate('After Auto-Review, map duplicate Content-Length normalization in the Example service request parser to determine which framing reaches the downstream body consumer.', 'request framing differential'),
              candidate('Bind Example service authorization subjects across reconnect and token refresh transitions to determine whether a stale principal can retain a newer session capability.', 'principal continuity across reconnect'),
              candidate('Compare Example service cache keys before and after URL canonicalization to identify two request identities that resolve to one privileged response object.', 'cache identity canonicalization'),
              candidate('Trace Example service upload staging through rename and cancellation to establish whether a validated temporary object can be replaced before privileged commit.', 'staged object replacement'),
              candidate('Correlate Example service retry identifiers with idempotency records to determine whether one failed mutation can be replayed under another caller context.', 'cross-caller idempotency replay'),
              candidate('Inspect Example service error serialization for reflected internal object identifiers that can be reused to address another tenant’s asynchronous operation.', 'error-channel object disclosure'),
              candidate('Follow Example service compression negotiation into decompression allocation accounting to distinguish wire-size limits from expanded-body enforcement.', 'compressed-size accounting'),
              candidate('Compare Example service webhook signature inputs with the normalized payload consumed by dispatch to isolate an authenticated-content interpretation split.', 'signature consumption differential')
            ]
          })
        };
      }
    };

    const generated = await generateStoredResearchGoalSuggestions(input, dependencies);
    assert.equal(generated.phase, 'discovery');
    assert.equal(generated.suggestions.length, 4);
    assert.equal(completions.length, 1);
    assert.equal(completions[0].provider, 'openai-codex');
    assert.equal(completions[0].model, 'gpt-test');
    assert.equal(completions[0].codexAuthFile, join(root, 'codex-auth.json'));
    assert.match(completions[0].systemPrompt, /Auto-Review is only a default, not an active setting/);
    assert.match(completions[0].systemPrompt, /Do not repeat, closely paraphrase/);
    assert.ok(generated.suggestions.every((suggestion) => !/Auto-Review/i.test(suggestion)));
    const prompt = JSON.parse(completions[0].prompt);
    assert.equal(prompt.workspace.id, 'workspace-test');
    assert.equal(prompt.candidateCount, 8);
    assert.ok(prompt.grounding.some((item) => item.id === 'resource:asset-test'));
    assert.ok(prompt.priorSuggestions.some((entry) => entry.suggestion.includes('legacy worker queue')));

    const legacyCache = new DatabaseSync(databasePath);
    legacyCache.prepare('UPDATE research_goal_suggestion_cache SET suggestions_json = ?')
      .run(JSON.stringify(generated.suggestions.map((suggestion) => `After Auto-Review, ${suggestion}`)));
    legacyCache.close();
    const cached = await generateStoredResearchGoalSuggestions(input, dependencies);
    assert.deepEqual(cached, generated);
    assert.equal(completions.length, 1);

    const database = new DatabaseSync(databasePath);
    const cache = database.prepare('SELECT profile_hash, scope_version_id FROM research_goal_suggestion_cache').get();
    const storedRevision = database.prepare('SELECT context_revision FROM research_goal_suggestion_cache').get();
    database.close();
    assert.match(storedRevision.context_revision, /^goal-suggestions-v2::/);
    selectStoredResearchGoalSuggestion({
      workspaceId: 'workspace-test',
      databasePath,
      scopeId: cache.scope_version_id,
      profileHash: cache.profile_hash,
      phase: 'discovery',
      suggestion: generated.suggestions[0]
    });

    const selectedDatabase = new DatabaseSync(databasePath, { readOnly: true });
    const selected = selectedDatabase.prepare(`SELECT selection_count FROM research_goal_suggestion_history
      WHERE workspace_id = ? AND scope_version_id = ? AND profile_hash = ? AND phase = ? AND suggestion_text = ?`)
      .get('workspace-test', cache.scope_version_id, cache.profile_hash, 'discovery', generated.suggestions[0]);
    const remaining = JSON.parse(selectedDatabase.prepare('SELECT suggestions_json FROM research_goal_suggestion_cache').get().suggestions_json);
    selectedDatabase.close();
    assert.equal(selected.selection_count, 1);
    assert.equal(remaining.length, 3);
    assert.ok(!remaining.includes(generated.suggestions[0]));

    let refreshPrompt;
    await assert.rejects(
      generateStoredResearchGoalSuggestions(
        { ...input, refresh: true },
        {
          completeText: async (options) => {
            refreshPrompt = JSON.parse(options.prompt);
            throw new Error('stop after prompt inspection');
          }
        }
      ),
      /stop after prompt inspection/
    );
    const selectedPrior = refreshPrompt.priorSuggestions.find(
      (entry) => entry.suggestion === generated.suggestions[0]
    );
    assert.equal(selectedPrior.selectionCount, 1);
    assert.ok(selectedPrior.selectedAt);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

function candidate(goal, noveltyAxis) {
  return {
    goal,
    groundingRefs: ['resource:asset-test'],
    rationale: 'The recorded Example service boundary makes this a concrete unresolved trust transition.',
    noveltyAxis
  };
}

function initializeSuggestionFixture(databasePath, workspaceRoot) {
  const database = new DatabaseSync(databasePath);
  database.exec(`
    PRAGMA foreign_keys = ON;
    CREATE TABLE workspaces (
      id TEXT PRIMARY KEY,
      workspace_path TEXT NOT NULL UNIQUE,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE scope_versions (
      id TEXT PRIMARY KEY,
      workspace_id TEXT REFERENCES workspaces(id),
      version INTEGER NOT NULL,
      status TEXT NOT NULL,
      workspace_name TEXT NOT NULL,
      scope_owner TEXT NOT NULL,
      description_markdown TEXT NOT NULL,
      rules_markdown TEXT NOT NULL,
      active_from TEXT NOT NULL,
      expires_at TEXT,
      created_at TEXT NOT NULL,
      created_by TEXT NOT NULL
    );
    CREATE TABLE scope_assets (
      id TEXT PRIMARY KEY,
      scope_version_id TEXT NOT NULL REFERENCES scope_versions(id),
      direction TEXT NOT NULL,
      kind TEXT NOT NULL,
      value TEXT NOT NULL,
      attributes_json TEXT NOT NULL,
      sensitivity TEXT NOT NULL,
      created_at TEXT NOT NULL
    );
    CREATE TABLE workspace_rules (
      id TEXT PRIMARY KEY,
      workspace_id TEXT NOT NULL REFERENCES workspaces(id),
      text TEXT NOT NULL,
      created_at TEXT NOT NULL,
      created_by TEXT NOT NULL
    );
    CREATE TABLE workspace_research_subjects (
      workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id),
      subject_id TEXT NOT NULL,
      display_name TEXT NOT NULL,
      source TEXT NOT NULL,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
    );
    CREATE TABLE research_goal_suggestion_cache (
      workspace_id TEXT NOT NULL REFERENCES workspaces(id),
      scope_version_id TEXT NOT NULL REFERENCES scope_versions(id),
      profile_hash TEXT NOT NULL,
      phase TEXT NOT NULL,
      context_revision TEXT NOT NULL,
      suggestions_json TEXT NOT NULL,
      generated_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      PRIMARY KEY (workspace_id, scope_version_id, profile_hash, phase)
    );
    CREATE TABLE research_goal_suggestion_history (
      workspace_id TEXT NOT NULL REFERENCES workspaces(id),
      scope_version_id TEXT NOT NULL REFERENCES scope_versions(id),
      profile_hash TEXT NOT NULL,
      phase TEXT NOT NULL,
      suggestion_key TEXT NOT NULL,
      suggestion_text TEXT NOT NULL,
      first_generated_at TEXT NOT NULL,
      last_generated_at TEXT NOT NULL,
      selected_at TEXT,
      generation_count INTEGER NOT NULL DEFAULT 1,
      selection_count INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY (workspace_id, scope_version_id, profile_hash, phase, suggestion_key)
    );
  `);
  const now = '2026-08-25T16:20:00.000Z';
  database.prepare('INSERT INTO workspaces VALUES (?, ?, ?, ?)')
    .run('workspace-test', workspaceRoot, now, now);
  database.prepare('INSERT INTO scope_versions VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)')
    .run('scope-test', 'workspace-test', 1, 'active', 'Test Workspace', 'Test Owner', 'Test scope', '', now, null, now, 'test');
  database.prepare('INSERT INTO scope_versions VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)')
    .run('scope-old', 'workspace-test', 0, 'retired', 'Test Workspace', 'Test Owner', 'Old scope', '', now, null, now, 'test');
  database.prepare('INSERT INTO scope_assets VALUES (?, ?, ?, ?, ?, ?, ?, ?)')
    .run('asset-test', 'scope-test', 'in_scope', 'service', 'https://example.test', '{}', 'public', now);
  database.prepare('INSERT INTO workspace_rules VALUES (?, ?, ?, ?, ?)')
    .run('rule-test', 'workspace-test', 'Stay within the recorded scope.', now, 'test');
  database.prepare('INSERT INTO workspace_research_subjects VALUES (?, ?, ?, ?, ?, ?)')
    .run('workspace-test', 'subject-test', 'Test Subject', 'explicit', now, now);
  database.prepare(`INSERT INTO research_goal_suggestion_history VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`)
    .run(
      'workspace-test',
      'scope-old',
      'old-profile-hash',
      'discovery',
      'legacy-key',
      'Map the legacy worker queue lease transition before ownership transfer.',
      now,
      now,
      null,
      1,
      0
    );
  database.close();
}
