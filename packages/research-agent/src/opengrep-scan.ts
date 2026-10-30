import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, realpath, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { ensurePortableBinary } from "./portable-binaries.js";
import { assertRepositorySourceRoot } from "./repository-source-root.js";

const MAX_OUTPUT_BYTES = 8 * 1024 * 1024;
const MAX_SCAN_MS = 60_000;

type JsonRecord = Record<string, unknown>;

export interface OpengrepMatch {
  path: string;
  line: number;
  column: number;
  endLine: number;
  ruleId: string;
  severity: string;
  message: string;
  code: string;
  enclosingContext: Array<{ kind: string; name: string; line: number }>;
  metavariables: Array<{ name: string; code: string }>;
  dataflow?: { source?: OpengrepFlowPoint; sink?: OpengrepFlowPoint; steps: OpengrepFlowPoint[] };
}

interface OpengrepFlowPoint { path: string; line: number; column: number; code: string; }

export interface OpengrepScanResult {
  matches: OpengrepMatch[];
  scannedFiles: number;
  totalMatches: number;
  issues: Array<{ type: string; message: string }>;
  partial: boolean;
  version: string;
}

function object(value: unknown): JsonRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as JsonRecord : null;
}
function string(value: unknown, max = 500): string {
  return typeof value === "string" ? value.slice(0, max) : "";
}
function positiveLine(value: unknown): number {
  return typeof value === "number" && Number.isInteger(value) && value > 0 ? value : 0;
}
function withinRoot(root: string, path: string): boolean {
  const pathFromRoot = relative(root, path);
  return pathFromRoot !== ".." && !pathFromRoot.startsWith(`..${sep}`) && !isAbsolute(pathFromRoot);
}

async function flowPoint(value: unknown, root: string): Promise<OpengrepFlowPoint | null> {
  const tuple = Array.isArray(value) && Array.isArray(value[1]) ? value[1] : null;
  const item = object(value);
  const location = tuple ? object(tuple[0]) : object(item?.location);
  if (!location || typeof location.path !== "string") return null;
  const absolutePath = await realpath(resolve(root, location.path)).catch(() => null);
  if (!absolutePath || !withinRoot(root, absolutePath)) return null;
  return { path: relative(root, absolutePath).split(sep).join("/"),
    line: positiveLine(object(location.start)?.line),
    column: positiveLine(object(location.start)?.col),
    code: string(tuple ? tuple[1] : item?.content, 300) };
}

async function parseMatch(value: unknown, root: string): Promise<OpengrepMatch | null> {
  const match = object(value);
  const extra = object(match?.extra);
  const start = object(match?.start);
  const end = object(match?.end);
  if (!match || !extra || !start || !end || typeof match.path !== "string") return null;
  const absolutePath = await realpath(resolve(root, match.path)).catch(() => null);
  if (!absolutePath || !withinRoot(root, absolutePath)) return null;
  const line = positiveLine(start.line);
  if (!line) return null;
  const contexts = Array.isArray(extra.enclosing_context) ? extra.enclosing_context : [];
  const metavars = object(extra.metavars);
  const trace = object(extra.dataflow_trace);
  const source = trace ? await flowPoint(trace.taint_source, root) : null;
  const sink = trace ? await flowPoint(trace.taint_sink, root) : null;
  const steps = trace && Array.isArray(trace.intermediate_vars)
    ? (await Promise.all(trace.intermediate_vars.slice(0, 8).map((step) => flowPoint(step, root))))
      .filter((step): step is OpengrepFlowPoint => step !== null) : [];
  return {
    path: relative(root, absolutePath).split(sep).join("/"),
    line,
    column: positiveLine(start.col),
    endLine: positiveLine(end.line) || line,
    ruleId: match.check_id === "-" ? "pattern" : string(match.check_id, 160),
    severity: string(extra.severity, 32),
    message: string(extra.message, 500),
    code: string(extra.lines, 800),
    enclosingContext: contexts.slice(0, 4).map((value) => {
      const context = object(value);
      return { kind: string(context?.kind, 40), name: string(context?.name, 160),
        line: positiveLine(object(context?.start)?.line) };
    }),
    metavariables: Object.entries(metavars ?? {}).slice(0, 12).map(([name, value]) => ({
      name: string(name, 80), code: string(object(value)?.abstract_content, 300),
    })),
    ...(trace ? { dataflow: { ...(source ? { source } : {}), ...(sink ? { sink } : {}), steps } } : {}),
  };
}

async function runProcess(executable: string, args: string[], root: string, signal?: AbortSignal): Promise<string> {
  const logDirectory = await mkdtemp(join(tmpdir(), "beale-opengrep-"));
  try {
    return await new Promise((resolveOutput, reject) => {
      const child = spawn(executable, args, { cwd: root, windowsHide: true,
        detached: process.platform !== "win32",
        stdio: ["ignore", "pipe", "pipe"],
        env: { ...process.env, SEMGREP_SEND_METRICS: "off", SEMGREP_APP_TOKEN: undefined,
          SEMGREP_RULES: undefined, OPENGREP_LOG_FILE: join(logDirectory, "scan.log"),
          SEMGREP_LOG_FILE: join(logDirectory, "scan.log") } });
      const output: Buffer[] = [];
      let outputBytes = 0;
      let stderr = "";
      let settled = false;
      let stopError: Error | undefined;
      let forceStop: ReturnType<typeof setTimeout> | undefined;
      const finish = (error?: Error) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        if (forceStop) clearTimeout(forceStop);
        signal?.removeEventListener("abort", abort);
        if (error) reject(error);
        else resolveOutput(Buffer.concat(output).toString("utf8"));
      };
      const killTree = (signal: NodeJS.Signals) => {
        if (process.platform === "win32" && child.pid !== undefined) {
          spawnSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "taskkill.exe"),
            ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore", timeout: 3_000 });
        } else if (child.pid !== undefined) {
          try { process.kill(-child.pid, signal); return; } catch { /* Process group already exited. */ }
        }
        child.kill(signal);
      };
      const stop = (error: Error) => {
        if (stopError) return;
        stopError = error;
        killTree("SIGTERM");
        forceStop = setTimeout(() => {
          killTree("SIGKILL");
          child.stdout.destroy();
          child.stderr.destroy();
          finish(error);
        }, 1_000);
        forceStop.unref();
      };
      const abort = () => stop(new Error("Repository scan was interrupted."));
      const timeout = setTimeout(() => stop(new Error("Repository scan timed out; narrow the root or pattern.")), MAX_SCAN_MS);
      child.stdout.on("data", (chunk: Buffer) => {
        outputBytes += chunk.byteLength;
        if (outputBytes > MAX_OUTPUT_BYTES) {
          stop(new Error("Repository scan output exceeded its limit; narrow the root or pattern."));
        } else output.push(chunk);
      });
      child.stderr.setEncoding("utf8");
      child.stderr.on("data", (chunk: string) => { stderr = (stderr + chunk).slice(-2_000); });
      child.once("error", (error) => finish(error));
      child.once("close", (code) => {
        if (stopError) finish(stopError);
        else if (code !== 0) finish(new Error(`Opengrep exited with code ${code}: ${stderr.trim().slice(-500)}`));
        else finish();
      });
      signal?.addEventListener("abort", abort, { once: true });
      if (signal?.aborted) abort();
    });
  } finally { await rm(logDirectory, { recursive: true, force: true }); }
}

export async function runRepositoryOpengrepScan(input: {
  root: string;
  pattern?: string;
  language?: string;
  rulesFile?: string;
  maxResults: number;
  executable?: string;
  signal?: AbortSignal;
}): Promise<OpengrepScanResult> {
  const root = await realpath(input.root);
  await assertRepositorySourceRoot(root);
  const rootStat = await stat(root);
  if (!rootStat.isDirectory()) throw new Error("Repository scan requires a directory root.");
  const patternMode = typeof input.pattern === "string" && input.pattern.trim().length > 0;
  if (patternMode === Boolean(input.rulesFile)) {
    throw new Error("Provide either pattern with language or a local rulesFile.");
  }
  if (patternMode && !input.language?.trim()) throw new Error("Pattern scans require a language.");
  if (!patternMode && input.language) throw new Error("language is only used with pattern scans.");
  const args = ["scan", "--json", "--quiet", "--disable-version-check", "--experimental",
    "--output-enclosing-context", "--dataflow-traces", "--jobs=2", "--timeout=5", "--max-target-bytes=1000000",
    "--max-match-per-file=100"];
  if (patternMode) {
    args.push("--pattern", input.pattern!, "--lang", input.language!);
  } else {
    const rulesFile = await realpath(resolve(root, input.rulesFile!)).catch(() => null);
    if (!rulesFile || !withinRoot(root, rulesFile) || !(await stat(rulesFile)).isFile()) {
      throw new Error("rulesFile must be an existing file inside the selected repository.");
    }
    args.push("--config", rulesFile);
  }
  args.push(".");
  const executable = input.executable ?? await ensurePortableBinary("opengrep", {
    ...(input.signal ? { signal: input.signal } : {}), deadline: Date.now() + MAX_SCAN_MS });
  if (!executable) throw new Error("The pinned Opengrep binary is unavailable on this host.");
  const output = await runProcess(executable, args, root, input.signal);
  const report = object(JSON.parse(output) as unknown);
  if (!report || !Array.isArray(report.results)) throw new Error("Opengrep returned an invalid JSON report.");
  const matches: OpengrepMatch[] = [];
  for (const result of report.results.slice(0, input.maxResults)) {
    const match = await parseMatch(result, root);
    if (match) matches.push(match);
  }
  const errors = Array.isArray(report.errors) ? report.errors : [];
  const skippedRules = Array.isArray(report.skipped_rules) ? report.skipped_rules : [];
  const issues = [...errors, ...skippedRules].slice(0, 10).map((value) => {
    const issue = object(value);
    return { type: string(issue?.type ?? issue?.rule_id, 80) || "scan",
      message: string(issue?.message ?? issue?.reason ?? issue?.details, 500) };
  });
  const scanned = object(report.paths)?.scanned;
  const scannedFiles = Array.isArray(scanned) ? scanned.length : 0;
  return { matches, scannedFiles,
    totalMatches: report.results.length, issues,
    partial: scannedFiles === 0 || report.results.length > input.maxResults
      || errors.length > 0 || skippedRules.length > 0,
    version: string(report.version, 80) };
}
