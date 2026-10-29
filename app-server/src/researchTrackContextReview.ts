export type ResearchTrackContextDecision = {
  mode: 'resume' | 'reset';
  reason: 'healthy' | 'capture_unavailable' | 'capture_oversized' | 'context_pressure' | 'repeated_compaction';
  estimatedTokens?: number;
  contextWindow?: number;
};

interface ContextEvent {
  kind?: unknown;
  timestamp?: unknown;
  agentPath?: unknown;
  payload?: unknown;
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown> : null;
}

function positiveNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null;
}

/** Review harness context signals from the last attempt before retaining its model history. */
export function reviewResearchTrackContext(input: {
  captureAvailable: boolean;
  attemptStartedAt?: string;
  events: readonly ContextEvent[];
}): ResearchTrackContextDecision {
  if (!input.captureAvailable) return { mode: 'reset', reason: 'capture_unavailable' };

  const startedAt = input.attemptStartedAt ? Date.parse(input.attemptStartedAt) : NaN;
  let estimatedTokens: number | null = null;
  let contextWindow: number | null = null;
  let fixedContextTokens = 0;
  let compactions = 0;
  let captureCompactedAfterEstimate = false;
  for (const event of input.events) {
    if (event.kind !== 'agent.event') continue;
    if (Number.isFinite(startedAt) && typeof event.timestamp === 'string'
      && Date.parse(event.timestamp) < startedAt) continue;
    const payload = record(event.payload);
    if (!payload || (event.agentPath ?? payload.agentPath ?? '/root') !== '/root') continue;
    if (payload.type === 'context_composed') {
      estimatedTokens = positiveNumber(payload.estimatedTokens) ?? estimatedTokens;
      const systemCharacters = positiveNumber(payload.systemPromptCharacters) ?? 0;
      const toolCharacters = positiveNumber(payload.toolDefinitionCharacters) ?? 0;
      fixedContextTokens = Math.ceil((systemCharacters + toolCharacters) / 4);
      captureCompactedAfterEstimate = false;
    } else if (payload.type === 'context_compacted') {
      compactions += 1;
      const messageTokens = positiveNumber(payload.tokensAfter);
      estimatedTokens = messageTokens === null ? estimatedTokens : messageTokens + fixedContextTokens;
      contextWindow = positiveNumber(payload.contextWindow) ?? contextWindow;
    } else if (payload.type === 'research_checkpoint'
      && (payload.reason === 'native' || payload.phase === 'resumable_capture')) {
      // A checkpoint can compact the saved transcript after the last composition metric.
      captureCompactedAfterEstimate = true;
    } else if (payload.type === 'turn_completed') {
      contextWindow = positiveNumber(payload.contextWindow) ?? contextWindow;
    }
  }
  const metrics = {
    ...(estimatedTokens !== null ? { estimatedTokens } : {}),
    ...(contextWindow !== null ? { contextWindow } : {}),
  };
  if (!captureCompactedAfterEstimate && estimatedTokens !== null && contextWindow !== null) {
    const occupancy = estimatedTokens / contextWindow;
    if (occupancy >= 0.82) return { mode: 'reset', reason: 'context_pressure', ...metrics };
    if (compactions >= 2 && occupancy >= 0.65) {
      return { mode: 'reset', reason: 'repeated_compaction', ...metrics };
    }
  }
  return { mode: 'resume', reason: 'healthy', ...metrics };
}
