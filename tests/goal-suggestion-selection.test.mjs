import assert from 'node:assert/strict';
import test from 'node:test';

import {
  parseAndSelectResearchGoalSuggestionCandidates,
  researchGoalSemanticSimilarity,
  researchGoalSuggestionCandidateCount
} from '../packages/research-agent/dist/index.js';

const workflow = {
  id: 'discovery',
  name: 'Discovery',
  description: 'Discover a new vulnerability direction.',
  goalSuggestionCount: 3,
  goalSuggestionInstructions: [],
  promptInstructions: [],
  outputRequirements: []
};

test('candidate selection rejects prior paraphrases and ranks distinct grounded goals', () => {
  const grounding = [{
    id: 'resource:service',
    kind: 'resource',
    title: 'Example gateway service',
    summary: 'An in-scope HTTP gateway with request parsing, caching, uploads, and webhook dispatch.'
  }];
  const prior = 'Trace duplicate HTTP Content-Length handling through the Example gateway parser and downstream request body consumer to find a framing discrepancy.';
  const output = JSON.stringify({ candidates: [
    candidate('Map duplicate Content-Length normalization in the Example gateway parser to determine which framing reaches the downstream request body consumer.', 'request framing differential'),
    candidate('Compare Example gateway cache keys before and after URL canonicalization to identify two request identities that resolve to one privileged response object.', 'cache identity canonicalization'),
    candidate('Trace Example gateway upload staging through rename and cancellation to establish whether a validated temporary object can be replaced before privileged commit.', 'staged object replacement'),
    candidate('Compare Example gateway webhook signature inputs with the normalized payload consumed by dispatch to isolate an authenticated-content interpretation split.', 'signature consumption differential'),
    candidate('Correlate Example gateway retry identifiers with idempotency records to determine whether one failed mutation can be replayed under another caller context.', 'cross-caller idempotency replay'),
    candidate('Follow Example gateway compression negotiation into decompression allocation accounting to distinguish wire-size limits from expanded-body enforcement.', 'compressed-size accounting')
  ] });

  const selection = parseAndSelectResearchGoalSuggestionCandidates({
    output,
    workflow,
    suggestionCount: 3,
    candidateCount: 6,
    grounding,
    priorSuggestions: [prior],
    previousResearchTexts: []
  });

  assert.equal(selection.rejectedPriorSuggestions, 1);
  assert.equal(selection.selected.length, 3);
  assert.ok(selection.selected.every((entry) => entry.groundingRefs.includes('resource:service')));
  assert.ok(!selection.selected.some((entry) => /Content-Length/.test(entry.goal)));
});

function candidate(goal, noveltyAxis) {
  return {
    goal,
    groundingRefs: ['resource:service'],
    rationale: 'The cited gateway boundary exposes a concrete unresolved state transition.',
    noveltyAxis
  };
}
