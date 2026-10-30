import { spawn } from "node:child_process";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { PRE_BEALE_DATA_DIRECTORY_NAME } from "./legacy-compatibility.js";
import { assertRepositorySourceRoot } from "./repository-source-root.js";

export interface RepositoryLiteralMatch { path: string; line: number; preview: string; }
export interface RepositoryLiteralSearchResult {
  matches: RepositoryLiteralMatch[];
  timedOut: boolean;
  partial: boolean;
  diagnostic?: string;
}

function textField(value: unknown): string | null {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return null;
  const text = (value as Record<string, unknown>).text;
  return typeof text === "string" ? text : null;
}

/** Stream bounded literal matches without loading candidate files into Node. */
export async function searchRepositoryWithRipgrep(input: {
  root: string;
  query: string;
  maxResults: number;
  maxFileBytes: number;
  deadline: number;
  signal?: AbortSignal;
}): Promise<RepositoryLiteralSearchResult> {
  if (input.signal?.aborted) throw new Error("Repository search was interrupted.");
  await assertRepositorySourceRoot(input.root);
  if (Date.now() >= input.deadline) return { matches: [], timedOut: true, partial: true };
  const { rgPath } = await import("@vscode/ripgrep");
  const excludedDirectories = [".git", ".hg", ".svn", ".beale",
    PRE_BEALE_DATA_DIRECTORY_NAME, "node_modules"];
  const args = ["--no-config", "--json", "--fixed-strings", "--ignore-case",
    `--max-filesize=${input.maxFileBytes}`, ...excludedDirectories.flatMap((name) => ["--glob", `!**/${name}/**`]),
    "--regexp", input.query, "--", "."];
  return new Promise((resolveSearch, reject) => {
    const child = spawn(rgPath, args, { cwd: input.root, windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"] });
    const matches: RepositoryLiteralMatch[] = [];
    let pending = "";
    let stderr = "";
    let settled = false;
    let timedOut = false;
    let reachedLimit = false;
    let parseError: Error | null = null;
    let skippedPath = false;
    let diagnostic: string | undefined;
    const finish = (error?: Error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      input.signal?.removeEventListener("abort", abort);
      if (error) reject(error);
      else resolveSearch({ matches, timedOut, partial: timedOut || skippedPath || Boolean(diagnostic),
        ...(diagnostic ? { diagnostic } : {}) });
    };
    const stop = () => { if (child.exitCode === null && child.signalCode === null) child.kill(); };
    const abort = () => { stop(); finish(new Error("Repository search was interrupted.")); };
    const timeout = setTimeout(() => { timedOut = true; stop(); }, Math.max(1, input.deadline - Date.now()));
    const consume = (line: string) => {
      if (!line || reachedLimit || parseError) return;
      let event: unknown;
      try { event = JSON.parse(line) as unknown; }
      catch { parseError = new Error("Ripgrep returned an invalid search event."); stop(); return; }
      if (event === null || typeof event !== "object" || Array.isArray(event)) return;
      const record = event as Record<string, unknown>;
      if (record.type !== "match" || !record.data || typeof record.data !== "object") return;
      const data = record.data as Record<string, unknown>;
      const path = textField(data.path);
      const lineText = textField(data.lines);
      if (!path || lineText === null || typeof data.line_number !== "number") {
        skippedPath = true;
        return;
      }
      const absolute = resolve(input.root, path);
      const relativePath = relative(input.root, absolute);
      if (relativePath === ".." || relativePath.startsWith(`..${sep}`) || isAbsolute(relativePath)) {
        skippedPath = true;
        return;
      }
      matches.push({ path: relativePath.split(sep).join("/"), line: data.line_number,
        preview: lineText.trim().slice(0, 240) });
      if (matches.length >= input.maxResults) { reachedLimit = true; clearTimeout(timeout); stop(); }
    };
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      pending += chunk;
      let newline = pending.indexOf("\n");
      while (newline >= 0 && !reachedLimit && !parseError) {
        consume(pending.slice(0, newline));
        pending = pending.slice(newline + 1);
        newline = pending.indexOf("\n");
      }
      if (!reachedLimit && pending.length > 1_000_000) {
        parseError = new Error("Ripgrep produced an oversized search event; narrow the root or query.");
        stop();
      }
    });
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => { stderr = (stderr + chunk).slice(-1_000); });
    child.once("error", (error) => finish(error));
    child.once("close", (code) => {
      if (parseError) return finish(parseError);
      if (!reachedLimit && pending.trim()) consume(pending.trim());
      if (parseError) return finish(parseError);
      if (timedOut) return finish();
      if (code === 0 || code === 1 || reachedLimit) return finish();
      const message = stderr.trim().slice(-500) || `exit code ${code}`;
      if (matches.length === 0) finish(new Error(`Repository search failed: ${message}`));
      else { diagnostic = message; finish(); }
    });
    input.signal?.addEventListener("abort", abort, { once: true });
    if (input.signal?.aborted) abort();
  });
}
