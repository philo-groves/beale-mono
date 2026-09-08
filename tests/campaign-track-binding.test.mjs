import assert from "node:assert/strict";
import test from "node:test";

import { campaignTrackBindingFromPrompt } from "../packages/research-agent/dist/index.js";

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
