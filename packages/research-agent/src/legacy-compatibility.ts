import { existsSync } from "node:fs";
import { resolve } from "node:path";

const PRE_BEALE_RUNTIME_ID = ["honey", "crisp"].join("");
const PRE_BEALE_RUNTIME_ENV_ID = PRE_BEALE_RUNTIME_ID.toUpperCase();

export const PRE_BEALE_DATA_DIRECTORY_NAME = `.${PRE_BEALE_RUNTIME_ID}`;

export function preBealeRuntimeId(): string {
  return PRE_BEALE_RUNTIME_ID;
}

export function preBealeHashDomain(suffix: string): string {
  return `${PRE_BEALE_RUNTIME_ID}:${suffix}`;
}

export function preBealeEnvironmentName(canonicalName: string): string {
  return canonicalName.replace("APP_SERVER", PRE_BEALE_RUNTIME_ENV_ID);
}

export function readCompatibleEnvironment(
  canonicalName: string,
  environment: NodeJS.ProcessEnv = process.env,
): string | undefined {
  return environment[canonicalName] ?? environment[preBealeEnvironmentName(canonicalName)];
}

export function installPreBealeEnvironmentAliases(
  environment: NodeJS.ProcessEnv = process.env,
): void {
  for (const [name, value] of Object.entries(environment)) {
    if (!value || !name.includes(PRE_BEALE_RUNTIME_ENV_ID)) continue;
    const canonicalName = name.replace(PRE_BEALE_RUNTIME_ENV_ID, "APP_SERVER");
    environment[canonicalName] ??= value;
  }
}

export function compatibleExistingPath(canonicalPath: string, previousPath: string): string {
  const canonical = resolve(canonicalPath);
  if (existsSync(canonical)) return canonical;
  const previous = resolve(previousPath);
  return existsSync(previous) ? previous : canonical;
}

export function readPreBealeRecord(
  record: Record<string, unknown> | null | undefined,
): Record<string, unknown> | undefined {
  const value = record?.[PRE_BEALE_RUNTIME_ID];
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

export function readCompatibleRecordValue(
  record: Record<string, unknown> | null | undefined,
  canonicalKey: string,
): unknown {
  if (!record) return undefined;
  if (Object.prototype.hasOwnProperty.call(record, canonicalKey)) return record[canonicalKey];
  return record[canonicalKey.replace("appServer", PRE_BEALE_RUNTIME_ID)];
}

export function adoptPreBealeRecordKeys(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(adoptPreBealeRecordKeys);
  if (value === null || typeof value !== "object") return value;

  return Object.fromEntries(Object.entries(value).map(([key, entry]) => [
    key.replace(PRE_BEALE_RUNTIME_ID, "appServer"),
    adoptPreBealeRecordKeys(entry),
  ]));
}
