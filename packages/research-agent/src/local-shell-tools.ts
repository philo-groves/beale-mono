import { accessSync, constants } from "node:fs";
import { delimiter, dirname, isAbsolute, join, resolve } from "node:path";

interface LocalShellToolResolutionOptions {
  signal?: AbortSignal;
  deadline?: number;
}

interface LocalShellToolSpec {
  name: string;
  commandNames: readonly string[];
  executable(options: LocalShellToolResolutionOptions): Promise<string | null>;
}

export interface LocalShellTool {
  name: string;
  executable: string;
}

export interface LocalShellToolFailure {
  name: string;
  failure: string;
}

type LocalShellToolResolution = LocalShellTool | LocalShellToolFailure | null;

// Add bundled tools here as they become available. Resolution is lazy so an
// omitted platform package cannot prevent the shell tool from starting.
const LOCAL_SHELL_TOOLS: readonly LocalShellToolSpec[] = [
  { name: "rg", commandNames: ["rg", "rg.exe"],
    executable: async () => (await import("@vscode/ripgrep")).rgPath },
  { name: "ipsw", commandNames: ["ipsw", "ipsw.exe"],
    executable: async (options) => (await import("./ipsw-binary.js")).ensureIpswExecutable(options) },
  { name: "jq", commandNames: ["jq", "jq.exe"],
    executable: async (options) => (await import("./portable-binaries.js")).ensurePortableBinary("jq", options) },
  { name: "fd", commandNames: ["fd", "fd.exe"],
    executable: async (options) => (await import("./portable-binaries.js")).ensurePortableBinary("fd", options) },
  { name: "opengrep", commandNames: ["opengrep", "opengrep.exe"],
    executable: async (options) => (await import("./portable-binaries.js")).ensurePortableBinary("opengrep", options) },
];

function specFor(name: string): LocalShellToolSpec | undefined {
  return LOCAL_SHELL_TOOLS.find((spec) => spec.commandNames.includes(name.toLowerCase()));
}

function executableAvailable(path: string): boolean {
  if (!isAbsolute(path)) return false;
  try { accessSync(path, constants.X_OK); return true; }
  catch { return false; }
}

async function resolveTool(spec: LocalShellToolSpec, options: LocalShellToolResolutionOptions): Promise<LocalShellToolResolution> {
  try {
    const executable = await spec.executable(options);
    return executable && executableAvailable(executable)
      ? { name: spec.name, executable }
      : { name: spec.name, failure: `A local ${spec.name} binary is unavailable for this host.` };
  } catch (error) {
    const message = error instanceof Error && /^(?:ipsw |The ipsw |Unsupported ipsw|Portable tool |fetch failed)/u.test(error.message)
      ? error.message : "The bundled tool could not be prepared.";
    return { name: spec.name, failure: `Local ${spec.name} provisioning failed: ${message}` };
  }
}

export async function bundledToolForBareUtility(
  utility: string, options: LocalShellToolResolutionOptions = {},
): Promise<LocalShellToolResolution> {
  if (utility.includes("/") || utility.includes("\\")) return null;
  const spec = specFor(utility);
  return spec ? resolveTool(spec, options) : null;
}

function commandCallsTool(command: string, spec: LocalShellToolSpec): boolean {
  const names = spec.commandNames.map((name) => name.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")).join("|");
  return new RegExp(`(?:^|[|;&(])\\s*(?:${names})(?=$|[\\s|;&<>])`, "imu").test(command);
}

function pathValue(environment: NodeJS.ProcessEnv): string {
  const key = Object.keys(environment).find((name) => name.toLowerCase() === "path");
  return key ? environment[key] ?? "" : process.platform === "win32" ? "" : "/usr/bin:/bin";
}

function toolOnPath(spec: LocalShellToolSpec, environment: NodeJS.ProcessEnv, cwd: string): boolean {
  const names = process.platform === "win32" ? spec.commandNames.filter((name) => name.endsWith(".exe"))
    : spec.commandNames.filter((name) => !name.endsWith(".exe"));
  return pathValue(environment).split(delimiter).some((directory) => names.some((name) =>
    executableAvailable(join(directory ? resolve(cwd, directory) : cwd, name))));
}

export async function bundledToolForMissingCommand(
  command: string, environment: NodeJS.ProcessEnv, cwd: string, options: LocalShellToolResolutionOptions = {},
): Promise<LocalShellToolResolution> {
  for (const spec of LOCAL_SHELL_TOOLS) {
    if (commandCallsTool(command, spec) && !toolOnPath(spec, environment, cwd)) {
      return resolveTool(spec, options);
    }
  }
  return null;
}

export async function bundledToolForFailedCommand(
  command: string, stderr: string, options: LocalShellToolResolutionOptions = {},
): Promise<LocalShellToolResolution> {
  // A second execution is safe only for a simple command beginning with the
  // missing utility. Compound commands are provisioned before their first run.
  if (/[|;&><\n\r`$()]/u.test(command)) return null;
  const first = /^\s*([^\s]+)(?:\s|$)/u.exec(command)?.[1]?.toLowerCase();
  const spec = first ? specFor(first) : undefined;
  if (!spec || !missingCommandMessage(stderr, spec)) return null;
  return resolveTool(spec, options);
}

function missingCommandMessage(stderr: string, spec: LocalShellToolSpec): boolean {
  return spec.commandNames.some((name) => {
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
    return new RegExp(`(?:^|\\n)[^\\n]*?\\b${escaped}: (?:command not found|not found)(?:\\s|$)`, "iu").test(stderr)
      || new RegExp(`The term ['\"]?${escaped}['\"]? is not recognized`, "iu").test(stderr);
  });
}

export function environmentWithBundledTool(environment: NodeJS.ProcessEnv, tool: LocalShellTool): NodeJS.ProcessEnv {
  const pathKey = Object.keys(environment).find((name) => name.toLowerCase() === "path") ?? "PATH";
  // POSIX login profiles can invoke standard host utilities before the tool
  // command runs. Keep those directories available with a narrow host PATH.
  const systemPath = process.platform === "win32" ? "" : `${delimiter}/usr/bin${delimiter}/bin`;
  return { ...environment, [pathKey]: `${dirname(tool.executable)}${delimiter}${pathValue(environment)}${systemPath}` };
}
