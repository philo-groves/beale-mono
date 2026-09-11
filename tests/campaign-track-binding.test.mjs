import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { CampaignTrackStore, campaignTrackBindingFromPrompt } from "../packages/research-agent/dist/index.js";

const TRACK_ID = "investigation_0123456789abcdef01234567";

test("mandatory prompt track binding resolves before a research session creates a new investigation", () => {
  assert.equal(campaignTrackBindingFromPrompt([
    "Continue the existing parser investigation:",
    "",
    `\`${TRACK_ID}\``,
    "",
    "TRACK BINDING IS MANDATORY",
    "",
    "Confirm investigation.status returns that identifier.",
  ].join("\n")), TRACK_ID);
});

test("explicit campaign-track fields bind while incidental identifiers do not", () => {
  assert.equal(campaignTrackBindingFromPrompt(`Campaign track: \`${TRACK_ID}\``), TRACK_ID);
  assert.equal(campaignTrackBindingFromPrompt(`Prior notes mention ${TRACK_ID}.`), null);
  assert.equal(campaignTrackBindingFromPrompt([
    `Investigation candidates: ${TRACK_ID}`,
    "investigation_89abcdef0123456701234567",
    "TRACK BINDING IS MANDATORY",
  ].join("\n")), null);
});

test("natural continuation language binds an existing canonical investigation", () => {
  assert.equal(
    campaignTrackBindingFromPrompt(`Resume the canonical investigation ${TRACK_ID} and continue its open proof.`),
    TRACK_ID,
  );
  assert.equal(
    campaignTrackBindingFromPrompt(`Use campaign track ${TRACK_ID} as the active continuation.`),
    TRACK_ID,
  );
});

test("session prompts resume the investigation referenced by durable resources without an investigation ID", async () => {
  const root = await mkdtemp(join(tmpdir(), "beale-track-binding-"));
  const store = new CampaignTrackStore({
    databasePath: join(root, "memory.sqlite"),
    context: { workspaceId: "workspace-example", workspaceName: "Example", subjectId: "subject-example", subjectName: "Example" },
  });
  try {
    const target = store.create({ title: "Parser proof", objective: "Establish the parser proof chain." });
    store.linkResource(target.id, "runbook", "runbook_0123456789abcdef", "proof");
    store.create({ title: "Unrelated review", objective: "Review a separate synthetic component." });
    const resumed = store.ensureForSession({
      sessionId: "session-example-continuation",
      objective: "Continue from runbook_0123456789abcdef and finish its pending proof.",
      source: "runtime",
      allowSimilarMatch: true,
    });
    assert.equal(resumed.id, target.id);
  } finally {
    store.close();
    await rm(root, { recursive: true, force: true });
  }
});
