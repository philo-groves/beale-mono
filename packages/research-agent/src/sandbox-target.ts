import { SandboxError, connectSandbox, createSandbox } from "@opencoredev/sandbox-sdk";
import type { Provider } from "@opencoredev/sandbox-sdk/provider";
import type { ShellSandboxRunner } from "./shell-tool.js";

/**
 * Sandbox SDK providers Fleet can offer as session targets. Each one needs no package beyond
 * `@opencoredev/sandbox-sdk`: the cloud providers call their HTTP APIs directly, and `local` drives
 * Docker on this host. Providers whose SDK is an optional peer package are left out until Beale
 * ships that package.
 */
export const FLEET_SANDBOX_PROVIDERS = ["e2b", "daytona", "freestyle", "superserve", "boat", "sandboxer", "local"] as const;
export type FleetSandboxProvider = (typeof FLEET_SANDBOX_PROVIDERS)[number];

/** Display names, and the environment variables each provider reads its credentials from. */
export const FLEET_SANDBOX_PROVIDER_DETAILS: Readonly<Record<FleetSandboxProvider, { name: string; credentials: readonly string[] }>> = {
  e2b: { name: "E2B", credentials: ["E2B_API_KEY"] },
  daytona: { name: "Daytona", credentials: ["DAYTONA_API_KEY"] },
  freestyle: { name: "Freestyle", credentials: ["FREESTYLE_API_KEY"] },
  superserve: { name: "Superserve", credentials: ["SUPERSERVE_API_KEY"] },
  boat: { name: "Boat", credentials: ["BOAT_API_KEY"] },
  sandboxer: { name: "Sandboxer", credentials: ["SANDBOXER_API_KEY"] },
  local: { name: "Docker", credentials: [] },
};

export function isFleetSandboxProvider(value: unknown): value is FleetSandboxProvider {
  return typeof value === "string" && (FLEET_SANDBOX_PROVIDERS as readonly string[]).includes(value);
}

/**
 * Creates the provider with its default options; each factory reads its credentials from the
 * environment. The import is dynamic so a session that never selects a sandbox target loads none
 * of the adapters.
 */
export async function fleetSandboxProvider(provider: FleetSandboxProvider): Promise<Provider> {
  switch (provider) {
    case "e2b": return (await import("@opencoredev/sandbox-sdk/e2b")).e2b();
    case "daytona": return (await import("@opencoredev/sandbox-sdk/daytona")).daytona();
    case "freestyle": return (await import("@opencoredev/sandbox-sdk/freestyle")).freestyle();
    case "superserve": return (await import("@opencoredev/sandbox-sdk/superserve")).superserve();
    case "boat": return (await import("@opencoredev/sandbox-sdk/boat")).boat();
    case "sandboxer": return (await import("@opencoredev/sandbox-sdk/sandboxer")).sandboxer();
    case "local": return (await import("@opencoredev/sandbox-sdk/local")).local();
  }
}

/** The serializable reference the app server hands a session worker for one sandbox target. */
export interface FleetSandboxReference {
  provider: FleetSandboxProvider;
  sandboxId: string;
}

/**
 * Returns a factory that connects to the sandbox on first use and reuses that handle for the
 * session. A failed connection is retried on the next command instead of being cached.
 */
export function fleetSandboxConnector(reference: FleetSandboxReference): () => Promise<ShellSandboxRunner> {
  let handle: Promise<ShellSandboxRunner> | null = null;
  return () => {
    handle ??= fleetSandboxProvider(reference.provider)
      // Beale never passes `secrets`, so skip the SDK's secret-recovery command before the first run.
      .then((provider) => connectSandbox(provider, reference.sandboxId, { recoverSecrets: false }))
      .catch((error: unknown) => { handle = null; throw error; });
    return handle;
  };
}

/** Resolves a provider by name. The app server takes one so tests can pass the SDK's `memory()` provider. */
export type FleetSandboxProviderResolver = (provider: FleetSandboxProvider) => Promise<Provider>;

/**
 * Gets or creates the sandbox for `key` and returns its ID, leaving it running for the session.
 * Reusing the key makes a retried or resumed launch find its sandbox instead of making another.
 */
export async function createFleetSandbox(provider: Provider, key: string, labels: Record<string, string>): Promise<string> {
  const box = await createSandbox(provider, { key, labels });
  try { return box.id; } finally { await box.close(); }
}

/** Deletes a session's sandbox. A sandbox that is already gone counts as deleted. */
export async function destroyFleetSandbox(provider: Provider, sandboxId: string): Promise<void> {
  try {
    await (await connectSandbox(provider, sandboxId)).destroy();
  } catch (error) {
    if (error instanceof SandboxError && error.code === "not_found") return;
    throw error;
  }
}
