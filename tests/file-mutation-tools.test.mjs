import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { createFileMutationTools, createResearchToolRegistry, createStructuredFileReadTool, initializeWorkspaceProject } from "../packages/research-agent/dist/index.js";

test("core file tools preserve exact bytes and reject stale, ambiguous, and protected writes", async () => {
  const directory = await mkdtemp(join(tmpdir(), "beale-file-tools-"));
  try {
    const protectedPath = join(directory, "host-managed.bin");
    await writeFile(protectedPath, "host-owned");
    const tools = createFileMutationTools({ workspaceRoot: directory, protectedPaths: [protectedPath] });
    const registry = createResearchToolRegistry([...tools, createStructuredFileReadTool({ contextRoots: [directory] })]);
    const call = async (name, input, options) => (await registry.execute({ id: "file-example", toolName: name, actionClass: name === "file.read" ? "inspect" : "synthesize", input }, options)).result;
    const original = "\ufeffalpha\r\nbeta\r\n";
    const created = await call("file.write", { path: "example.txt", content: original });
    assert.equal(created.status, "complete");
    assert.equal(created.output.candidate, true);
    const read = await call("file.read", { path: join(directory, "example.txt"), maxBytes: 5 });
    assert.equal(read.output.contentHash, createHash("sha256").update(original).digest("hex"));
    assert.equal(read.modelOutput.contentHash, created.output.contentHash);
    assert.equal((await call("file.write", { path: "example.txt", content: "replace" })).status, "error");
    assert.equal((await call("file.edit", { path: "example.txt", oldText: "alpha", newText: "gamma", expectedHash: created.output.contentHash })).status, "complete");
    assert.equal(await readFile(join(directory, "example.txt"), "utf8"), "\ufeffgamma\r\nbeta\r\n");
    assert.equal((await call("file.write", { path: "example.txt", content: "stale", expectedHash: created.output.contentHash })).status, "error");
    assert.equal((await call("file.edit", { path: "example.txt", oldText: "\r\n", newText: "" })).status, "error");
    assert.equal((await call("file.edit", { path: protectedPath, oldText: "host", newText: "client" })).status, "error");
    assert.equal(await readFile(protectedPath, "utf8"), "host-owned");
    const denied = await call("file.write", { path: "denied.txt", content: "example" }, { governance: { deniedSideEffects: ["write"] } });
    assert.equal(denied.status, "blocked");
    const budgeted = await call("file.write", { path: "budgeted.txt", content: "example" }, { governance: { maxBytes: 20 } });
    assert.equal(budgeted.status, "complete");
    assert.equal((await call("file.write", { path: "over-budget.txt", content: "example" }, { governance: { maxBytes: 3 } })).status, "error");
    await assert.rejects(readFile(join(directory, "denied.txt")), { code: "ENOENT" });
    await writeFile(join(directory, "large.txt"), Buffer.alloc(1_048_577, 65));
    assert.equal((await call("file.edit", { path: "large.txt", oldText: "A", newText: "B" })).status, "error");
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test('managed file tools enforce categories, retain overwritten bytes, and protect artifact descendants', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'beale-managed-files-'));
  try {
    const root = join(directory, 'workspace');
    initializeWorkspaceProject(root, 'workspace-example');
    const artifacts = join(directory, 'artifacts');
    await mkdir(artifacts);
    await writeFile(join(artifacts, 'example.txt'), 'canonical artifact');
    const registry = createResearchToolRegistry(createFileMutationTools({ workspaceRoot: root, protectedPaths: [artifacts] }));
    const call = async (name, input) => (await registry.execute({ id: 'file-example', toolName: name, actionClass: 'synthesize', input })).result;
    assert.equal((await call('file.write', { path: 'loose.py', content: 'example' })).status, 'error');
    assert.equal((await call('file.write', { path: 'claims/example.json', content: '{}' })).status, 'error');
    assert.equal((await call('file.edit', { path: join(artifacts, 'example.txt'), oldText: 'canonical', newText: 'changed' })).status, 'error');
    const created = await call('file.write', { path: 'investigations/example.txt', content: 'original' });
    assert.equal(created.status, 'complete');
    assert.equal((await call('file.write', { path: 'investigations/example.txt', content: 'replacement', expectedHash: created.output.contentHash })).status, 'complete');
    assert.equal(await readFile(join(root, '.git', 'beale', 'recovery', created.output.contentHash), 'utf8'), 'original');
    assert.equal((await call('file.write', { path: 'scratch/example.txt', content: 'disposable' })).status, 'complete');
  } finally { await rm(directory, { recursive: true, force: true }); }
});
