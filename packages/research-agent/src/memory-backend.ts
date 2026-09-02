export const RESEARCH_MEMORY_BACKEND_IDS = ["app-server", "disabled"] as const;

export type ResearchMemoryBackendId = (typeof RESEARCH_MEMORY_BACKEND_IDS)[number];

export interface ResearchMemoryBackend {
  id: ResearchMemoryBackendId;
  enabled: boolean;
}

const BACKENDS: Record<ResearchMemoryBackendId, ResearchMemoryBackend> = {
  "app-server": { id: "app-server", enabled: true },
  disabled: { id: "disabled", enabled: false },
};

export function isResearchMemoryBackendId(value: unknown): value is ResearchMemoryBackendId {
  return typeof value === "string" && (RESEARCH_MEMORY_BACKEND_IDS as readonly string[]).includes(value);
}

export function resolveResearchMemoryBackend(value: unknown): ResearchMemoryBackend {
  // Stored v1/v2 selections all converge on the single in-place app-server schema.
  const previousId = preBealeRuntimeId();
  const id = value === undefined
    || value === previousId
    || value === `${previousId}-v1`
    || value === `${previousId}-v2-shadow`
    || value === `${previousId}-v2`
    || value === "app-server-v1"
    || value === "app-server-v2-shadow"
    || value === "app-server-v2"
    ? "app-server"
    : value;
  if (!isResearchMemoryBackendId(id)) {
    throw new Error(`Unsupported research memory backend: ${String(id)}`);
  }
  return BACKENDS[id];
}
import { preBealeRuntimeId } from "./legacy-compatibility.js";
