export const RESEARCH_MEMORY_BACKEND_IDS = ["honeycrisp", "disabled"] as const;

export type ResearchMemoryBackendId = (typeof RESEARCH_MEMORY_BACKEND_IDS)[number];

export interface ResearchMemoryBackend {
  id: ResearchMemoryBackendId;
  enabled: boolean;
}

const BACKENDS: Record<ResearchMemoryBackendId, ResearchMemoryBackend> = {
  honeycrisp: { id: "honeycrisp", enabled: true },
  disabled: { id: "disabled", enabled: false },
};

export function isResearchMemoryBackendId(value: unknown): value is ResearchMemoryBackendId {
  return typeof value === "string" && (RESEARCH_MEMORY_BACKEND_IDS as readonly string[]).includes(value);
}

export function resolveResearchMemoryBackend(value: unknown): ResearchMemoryBackend {
  // Stored v1/v2 selections all converge on the single in-place Honeycrisp schema.
  const id = value === undefined || value === "honeycrisp-v1" || value === "honeycrisp-v2-shadow" || value === "honeycrisp-v2"
    ? "honeycrisp"
    : value;
  if (!isResearchMemoryBackendId(id)) {
    throw new Error(`Unsupported research memory backend: ${String(id)}`);
  }
  return BACKENDS[id];
}
