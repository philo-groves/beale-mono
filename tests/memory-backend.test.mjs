import assert from "node:assert/strict";
import test from "node:test";
import {
  RESEARCH_MEMORY_BACKEND_IDS,
  isResearchMemoryBackendId,
  resolveResearchMemoryBackend,
} from "../packages/research-agent/dist/index.js";

test("resolves one canonical workspace memory backend and migrates legacy IDs", () => {
  assert.deepEqual(RESEARCH_MEMORY_BACKEND_IDS, ["honeycrisp", "disabled"]);
  assert.deepEqual(resolveResearchMemoryBackend(), { id: "honeycrisp", enabled: true });
  assert.deepEqual(resolveResearchMemoryBackend("disabled"), { id: "disabled", enabled: false });
  assert.equal(isResearchMemoryBackendId("honeycrisp"), true);
  assert.equal(isResearchMemoryBackendId("honeycrisp-v1"), false);
  assert.deepEqual(resolveResearchMemoryBackend("honeycrisp-v1"), { id: "honeycrisp", enabled: true });
  assert.deepEqual(resolveResearchMemoryBackend("honeycrisp-v2-shadow"), { id: "honeycrisp", enabled: true });
  assert.deepEqual(resolveResearchMemoryBackend("honeycrisp-v2"), { id: "honeycrisp", enabled: true });
  assert.equal(isResearchMemoryBackendId("future-v2"), false);
  assert.throws(() => resolveResearchMemoryBackend("future-v2"), /Unsupported research memory backend/);
});
