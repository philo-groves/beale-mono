import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, unlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test } from "node:test";

const temporaryDirectories = [];
const children = new Set();

afterEach(async () => {
  for (const child of children) {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGTERM");
    await Promise.race([childExit(child), delay(2_000)]);
  }
  children.clear();
  for (const directory of temporaryDirectories.splice(0)) {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("headless app-server exits after losing its discovery record", async () => {
  const directory = mkdtempSync(join(tmpdir(), "beale-headless-lifetime-"));
  temporaryDirectories.push(directory);
  const stateFile = join(directory, "app-server.json");
  const child = spawn(process.execPath, ["app-server/dist/headlessMain.js", "--state-file", stateFile], {
    cwd: new URL("../..", import.meta.url),
    env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" },
    stdio: ["ignore", "ignore", "pipe"],
    windowsHide: true,
  });
  children.add(child);
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr += chunk; });

  await waitFor(() => existsSync(stateFile), 5_000);
  unlinkSync(stateFile);
  const result = await Promise.race([
    childExit(child),
    delay(4_000).then(() => null),
  ]);

  assert.ok(result, `headless app-server did not exit after losing discovery ownership: ${stderr}`);
  assert.equal(result.code, 0);
});

function childExit(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve({ code: child.exitCode, signal: child.signalCode });
  }
  return new Promise((resolve) => child.once("exit", (code, signal) => resolve({ code, signal })));
}

async function waitFor(predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await delay(25);
  }
  throw new Error("Timed out waiting for the headless app-server.");
}

function delay(milliseconds) {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, milliseconds);
    timer.unref();
  });
}
