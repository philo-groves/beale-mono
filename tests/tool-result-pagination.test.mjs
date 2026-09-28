import assert from "node:assert/strict";
import test from "node:test";

import {
  createResearchToolRegistry,
  projectModelToolResult,
} from "../packages/research-agent/dist/index.js";

const syntheticResult = "A".repeat(17_000) + "EXAMPLECO_OMITTED_MIDDLE" + "B".repeat(17_000);

function makeTool(output = syntheticResult, modelOutput) {
  return {
    descriptor: {
      name: "fixture.inspect",
      description: "Return synthetic inspection output.",
      actionClasses: ["inspect"],
      sideEffects: "none",
      requiredPermissions: [],
      inputSchema: { type: "object", additionalProperties: false },
    },
    async execute(action) {
      const timestamp = new Date().toISOString();
      return {
        action,
        status: "complete",
        startedAt: timestamp,
        completedAt: timestamp,
        summary: "Inspected a synthetic example.",
        output,
        ...(modelOutput === undefined ? {} : { modelOutput }),
        followUpActions: [],
      };
    },
  };
}

async function runInspection(registry, agentId = "agent-example") {
  return registry.execute({
    id: "action-example-001",
    toolName: "fixture.inspect",
    actionClass: "inspect",
    input: {},
  }, { agentId });
}

async function readPage(registry, offset, agentId = "agent-example", maxCharacters = 8_000) {
  return registry.execute({
    id: `page-example-${offset}`,
    toolName: "tool_result_page",
    actionClass: "inspect",
    input: { toolActionId: "action-example-001", offset, maxCharacters },
  }, { agentId });
}

test("truncated tool results expose bounded pages through a forked registry", async () => {
  const registry = createResearchToolRegistry([makeTool()], { paginateResults: true });
  assert.ok(registry.listDescriptors().some((descriptor) => descriptor.name === "tool_result.page"));
  const inspection = await runInspection(registry);
  const preview = projectModelToolResult(inspection.result).content[0].text;
  assert.ok(preview.length < 33_000);
  assert.match(preview, /tool_result_page.*action-example-001/);
  assert.doesNotMatch(preview, /EXAMPLECO_OMITTED_MIDDLE/);

  const fork = registry.fork();
  let offset = inspection.result.pagination.omittedStart;
  let omitted = "";
  while (offset < inspection.result.pagination.omittedEnd) {
    const page = await readPage(fork, offset);
    assert.equal(page.result.status, "complete");
    assert.ok(page.result.output.text.length <= 8_000);
    assert.equal(page.result.output.offset, offset);
    assert.equal(page.result.output.toolActionId, inspection.action.id);
    assert.doesNotMatch(projectModelToolResult(page.result).content[0].text, /Tool result truncated/);
    omitted += page.result.output.text;
    offset = page.result.output.nextOffset ?? page.result.output.totalCharacters;
  }
  const serialized = JSON.stringify({
    status: "complete",
    summary: "Inspected a synthetic example.",
    output: syntheticResult,
    followUpActions: [],
  }, null, 2);
  assert.equal(omitted.slice(0, inspection.result.pagination.omittedEnd - inspection.result.pagination.omittedStart),
    serialized.slice(inspection.result.pagination.omittedStart, inspection.result.pagination.omittedEnd));
  assert.match(omitted, /EXAMPLECO_OMITTED_MIDDLE/);
});

test("pages are scoped to the agent and reject invalid offsets", async () => {
  const registry = createResearchToolRegistry([makeTool()], { paginateResults: true });
  await runInspection(registry);
  const otherAgent = await readPage(registry, 0, "agent-other");
  assert.equal(otherAgent.result.status, "error");
  const invalidOffset = await readPage(registry, 99_999);
  assert.equal(invalidOffset.result.status, "error");
});

test("pagination retains only the model-visible projection and skips small results", async () => {
  const rawMarker = "RAW_PRIVATE_EXAMPLE_OUTPUT";
  const modelMarker = "EXAMPLECO_MODEL_VISIBLE_MIDDLE";
  const registry = createResearchToolRegistry([
    makeTool(rawMarker.repeat(2_000), "C".repeat(17_000) + modelMarker + "D".repeat(17_000)),
  ], { paginateResults: true });
  const inspection = await runInspection(registry);
  const page = await readPage(registry, inspection.result.pagination.omittedStart);
  assert.equal(page.result.status, "complete");
  assert.doesNotMatch(JSON.stringify(page.result.output), /RAW_PRIVATE_EXAMPLE_OUTPUT/);
  assert.match(page.result.output.text, /EXAMPLECO_MODEL_VISIBLE_MIDDLE/);

  const smallRegistry = createResearchToolRegistry([makeTool("small example")], { paginateResults: true });
  const small = await runInspection(smallRegistry);
  assert.equal(small.result.pagination, undefined);
  assert.doesNotMatch(projectModelToolResult(small.result).content[0].text, /tool_result_page/);
  assert.equal((await readPage(smallRegistry, 0)).result.status, "error");
});
