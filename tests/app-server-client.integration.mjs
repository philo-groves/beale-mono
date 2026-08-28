import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { researchProfileHash } from "../packages/research-agent/dist/index.js";

const clientPath = fileURLToPath(new URL("../packages/honeycrisp-host/dist/cli.js", import.meta.url));

test("the optional client resolves profiles through the app-server", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "beale-profile-client-"));
  try {
    const result = runClient([
      "profile", "resolve", "--workspace-root", workspaceRoot,
      "--profile-id", "mathematics", "--json",
    ]);
    assert.equal(result.status, 0, result.stderr);
    const envelope = JSON.parse(result.stdout);
    assert.equal(envelope.catalogProtocolVersion, 1);
    assert.equal(envelope.source, "bundled-default");
    assert.equal(envelope.profile.id, "mathematics");
    assert.equal(envelope.hash, researchProfileHash(envelope.profile));
  } finally {
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("the optional client cannot execute a research session locally", () => {
  const result = runClient(["-p", "Run outside the app-server"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /execution is hosted by the Beale app-server/);
});

test("the optional client reads model catalogs through the app-server", () => {
  const result = runClient(["models", "list", "openai-codex", "--json"]);
  assert.equal(result.status, 0, result.stderr);
  const catalog = JSON.parse(result.stdout);
  assert.equal(catalog.providers[0].providerId, "openai-codex");
  assert.ok(catalog.providers[0].models.length > 0);
});

function runClient(args) {
  return spawnSync(process.execPath, [clientPath, ...args], {
    encoding: "utf8",
    env: { ...process.env, NO_COLOR: "1", NODE_NO_WARNINGS: "1" },
  });
}
