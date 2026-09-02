import assert from "node:assert/strict";
import test from "node:test";
import {
  RESEARCH_MEMORY_BACKEND_IDS,
  isResearchMemoryBackendId,
  resolveResearchMemoryBackend,
} from "../packages/research-agent/dist/index.js";

test("resolves one canonical workspace memory backend and migrates legacy IDs", () => {
  const previousId = ["honey", "crisp"].join("");
  assert.deepEqual(RESEARCH_MEMORY_BACKEND_IDS, ["app-server", "disabled"]);
  assert.deepEqual(resolveResearchMemoryBackend(), { id: "app-server", enabled: true });
  assert.deepEqual(resolveResearchMemoryBackend("disabled"), { id: "disabled", enabled: false });
  assert.equal(isResearchMemoryBackendId("app-server"), true);
  assert.equal(isResearchMemoryBackendId("app-server-v1"), false);
  assert.deepEqual(resolveResearchMemoryBackend("app-server-v1"), { id: "app-server", enabled: true });
  assert.deepEqual(resolveResearchMemoryBackend("app-server-v2-shadow"), { id: "app-server", enabled: true });
  assert.deepEqual(resolveResearchMemoryBackend("app-server-v2"), { id: "app-server", enabled: true });
  assert.deepEqual(resolveResearchMemoryBackend(previousId), { id: "app-server", enabled: true });
  assert.deepEqual(resolveResearchMemoryBackend(`${previousId}-v2-shadow`), { id: "app-server", enabled: true });
  assert.equal(isResearchMemoryBackendId("future-v2"), false);
  assert.throws(() => resolveResearchMemoryBackend("future-v2"), /Unsupported research memory backend/);
});
