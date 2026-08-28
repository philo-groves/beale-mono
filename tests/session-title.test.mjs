import assert from "node:assert/strict";
import test from "node:test";

import {
  generateResearchSessionTitle,
  normalizeResearchSessionTitle,
} from "../packages/research-agent/dist/index.js";

test("Anthropic session titles always use the Claude Agent SDK completion route", async () => {
  const calls = [];
  const title = await generateResearchSessionTitle({
    provider: "anthropic",
    model: "claude-haiku-4-5",
    prompt: "Inspect a parser state transition.",
    models: {
      getModel() {
        assert.fail("Anthropic titles must not resolve a Pi model.");
      },
      async completeSimple() {
        assert.fail("Anthropic titles must not use Pi completion.");
      },
    },
    async completeClaudeText(options) {
      calls.push(options);
      return { text: "Parser State Transition", usage: { inputTokens: 12 } };
    },
  });

  assert.equal(title, "Parser State Transition");
  assert.equal(calls.length, 1);
  assert.equal(calls[0]?.model, "claude-haiku-4-5");
  assert.match(calls[0]?.prompt, /parser state transition/i);
});

test("research session titles retry transient provider failures", async () => {
  let attempts = 0;
  const title = await generateResearchSessionTitle({
    provider: "openai-codex",
    model: "gpt-5.6-luna",
    prompt: "Investigate the Erdős-Straus conjecture.",
    models: {
      getModel() { return { provider: "openai-codex", id: "gpt-5.6-luna" }; },
      async completeSimple() {
        attempts += 1;
        if (attempts === 1) {
          return {
            role: "assistant",
            content: [],
            api: "fixture",
            provider: "openai-codex",
            model: "gpt-5.6-luna",
            usage: {},
            stopReason: "error",
            errorMessage: "Our servers are currently overloaded. Please try again later.",
            timestamp: Date.now(),
          };
        }
        return {
          role: "assistant",
          content: [{ type: "text", text: "Erdős-Straus Conjecture Investigation" }],
          api: "fixture",
          provider: "openai-codex",
          model: "gpt-5.6-luna",
          usage: {},
          stopReason: "stop",
          timestamp: Date.now(),
        };
      },
    },
  });

  assert.equal(title, "Erdős-Straus Conjecture Investigation");
  assert.equal(attempts, 2);
});

test("research session titles use profile vocabulary", async () => {
  let systemPrompt = "";
  await generateResearchSessionTitle({
    provider: "fixture",
    model: "fixture",
    prompt: "Compare the two sediment chronologies.",
    researchProfile: {
      name: "Climate History",
      workspace: { subjectNoun: "Field site" },
      presentation: { sessionLabel: "Study Session" },
    },
    models: {
      getModel() { return { provider: "fixture", id: "fixture" }; },
      async completeSimple(_model, context) {
        systemPrompt = context.systemPrompt;
        return {
          role: "assistant",
          content: [{ type: "text", text: "Sediment Chronology Comparison" }],
          api: "fixture",
          provider: "fixture",
          model: "fixture",
          usage: {},
          stopReason: "stop",
          timestamp: Date.now(),
        };
      },
    },
  });

  assert.match(systemPrompt, /Climate History study session/);
  assert.match(systemPrompt, /field site/i);
  assert.doesNotMatch(systemPrompt, /security research/i);
});
