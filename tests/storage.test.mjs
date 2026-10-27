import assert from "node:assert/strict";
import { mkdir, mkdtemp, open, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  createResearchStorageLayout,
  ensureResearchStorageLayout,
  getResearchStorageManifestPath,
  listResearchStorageArtifacts,
  loadResearchStorageManifest,
  registerResearchStorageArtifact,
  resolveResearchStorageArtifact,
} from "../packages/research-agent/dist/index.js";

test("storage manifest registers, lists, resolves, and reloads artifacts", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-storage-"));
  const outsideRoot = await mkdtemp(join(tmpdir(), "app-server-storage-outside-"));
  const layout = ensureResearchStorageLayout(
    createResearchStorageLayout({ workspaceRoot }),
  );
  assert.deepEqual(layout.directories.map((directory) => directory.name), ["artifacts"]);
  const artifactPath = join(layout.artifactDirectoryPath, "reports", "summary.txt");
  const outsidePath = join(outsideRoot, "outside.txt");
  await mkdir(join(layout.artifactDirectoryPath, "reports"), { recursive: true });
  await writeFile(artifactPath, "stored report\n", "utf8");
  await writeFile(outsidePath, "outside\n", "utf8");

  try {
    const entry = registerResearchStorageArtifact(layout, {
      path: artifactPath,
      kind: "report",
      purpose: "Phase 2 manifest test report.",
      sourceEventIds: ["evt_alpha"],
    });
    const manifest = loadResearchStorageManifest(layout);
    const listed = listResearchStorageArtifacts(layout, {
      kind: "report",
      sourceEventId: "evt_alpha",
    });
    const resolved = resolveResearchStorageArtifact(layout, entry.id);

    assert.equal(manifest.manifestPath, getResearchStorageManifestPath(layout));
    assert.equal(manifest.artifacts.length, 1);
    if (process.platform !== "win32") {
      assert.equal((await stat(manifest.manifestPath)).mode & 0o777, 0o600);
    }
    assert.equal(listed.length, 1);
    assert.equal(resolved?.id, entry.id);
    assert.equal(entry.relativePath, "artifacts/reports/summary.txt");
    assert.match(entry.contentHash, /^sha256:[a-f0-9]{64}$/);
    assert.throws(
      () =>
        registerResearchStorageArtifact(layout, {
          path: outsidePath,
          kind: "outside",
        }),
      /under storage root/,
    );
  } finally {
    await rm(workspaceRoot, { recursive: true, force: true });
    await rm(outsideRoot, { recursive: true, force: true });
  }
});

test("storage manifest reports incomplete JSON without replacing the damaged file", async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-storage-incomplete-"));
  try {
    const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
    const manifestPath = getResearchStorageManifestPath(layout);
    await writeFile(manifestPath, "", "utf8");
    assert.throws(() => loadResearchStorageManifest(layout), /manifest contains incomplete or invalid JSON/u);
    assert.equal(await readFile(manifestPath, "utf8"), "");
  } finally {
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});

test("storage manifest replaces a complete file without truncating existing readers", {
  skip: process.platform === "win32" ? "Windows cannot rename over a file held open by this reader" : false,
}, async () => {
  const workspaceRoot = await mkdtemp(join(tmpdir(), "app-server-storage-atomic-"));
  try {
    const layout = ensureResearchStorageLayout(createResearchStorageLayout({ workspaceRoot }));
    const artifactPath = join(layout.artifactDirectoryPath, "report.txt");
    const secondArtifactPath = join(layout.artifactDirectoryPath, "second-report.txt");
    await writeFile(artifactPath, "synthetic report\n", "utf8");
    await writeFile(secondArtifactPath, "another synthetic report\n", "utf8");
    registerResearchStorageArtifact(layout, { path: artifactPath, kind: "report" });
    const reader = await open(getResearchStorageManifestPath(layout), "r");
    try {
      registerResearchStorageArtifact(layout, { path: secondArtifactPath, kind: "report" });
      const before = JSON.parse(await reader.readFile("utf8"));
      const after = JSON.parse(await readFile(getResearchStorageManifestPath(layout), "utf8"));
      assert.equal(before.artifacts.length, 1);
      assert.equal(after.artifacts.length, 2);
    } finally {
      await reader.close();
    }
  } finally {
    await rm(workspaceRoot, { recursive: true, force: true });
  }
});
