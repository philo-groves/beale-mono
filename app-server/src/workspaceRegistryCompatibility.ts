import { readFileSync } from 'node:fs';
import { join } from 'node:path';

export const MEMORY_NODE_TYPES = [
  'asset', 'bug', 'invariant', 'mitigation', 'source', 'sink',
  'hypothesis', 'primitive', 'chain', 'procedure', 'trajectory',
] as const;

export const DEFAULT_MEMORY_TYPE_DESCRIPTIONS = Object.freeze({
  asset: 'A security-relevant component, service, data object, credential, interface, or execution boundary whose compromise or protection matters. Use it to anchor affected ownership and impact; do not use it for arbitrary files with no security role.',
  bug: 'A confirmed historical flaw precedent that predates the current research, backed by a fixed advisory, patch, prior incident, or equivalent evidence. It must identify affected assets and set attributes.historicalPrecedent=true; a flaw established during the current research is a primitive, not a bug.',
  invariant: 'A security property that must remain true across relevant states or transitions. State it as a falsifiable rule whose violation would create security impact, not as a one-off observation.',
  mitigation: 'A concrete product, platform, hardware, policy, or deployment control that prevents or materially constrains exploitation. Record what it blocks and its assumptions; an ordinary validation step is not automatically a mitigation.',
  source: 'An attacker-controlled or lower-trust ingress from which data, control, identity, or state enters the investigated system. Name the trust boundary and reachable input, not merely a function that reads bytes.',
  sink: 'A security-sensitive operation or state transition whose unsafe reachability can produce impact, such as memory access, code execution, authorization, disclosure, or persistence. Name the dangerous effect and required conditions.',
  hypothesis: 'A specific, testable, currently unproven security proposition. Keep it draft or suspected while active, reject it when disproven, and reclassify it as a primitive or chain when evidence proves that role; never confirm a hypothesis in place. For a flaw hypothesis, record the suspected mechanism in attributes.rootCause and a stable lowercase-hyphenated attributes.rootCauseKey.',
  primitive: 'One independently proven security flaw or exploitation capability established during the current research, with direct code, artifact, command, or verifier evidence. Store the underlying root-cause mechanism, not each symptom, experiment, call site, or copy path, as the unit of identity; record attributes.rootCause and a stable lowercase-hyphenated attributes.rootCauseKey.',
  chain: 'An end-to-end attacker path linking one or more primitives to demonstrated security impact. Record reachability and affected context; source, sink, and asset relationships are ideal when supported but are not required. A confirmed chain requires proof-of-vulnerability evidence and independent review approval; do not use chain for an isolated flaw or an unlinked list of observations. Record its mechanism in attributes.rootCause and a stable lowercase-hyphenated attributes.rootCauseKey.',
  procedure: 'A concise, reusable operational method for performing a bounded research task or verification. Store essential prerequisites and decision points; use a runbook for an executable multi-step command sequence or environment setup.',
  trajectory: 'A reusable sequence of significant research choices and results that explains how an investigation advanced or why a path failed. Omit routine narration and transcripts; preserve the discriminating steps and outcome.',
});

const RESEARCH_KIT_IDS = new Set(['general', 'hackerone', 'apple-security-bounty', 'google-oss-vrp', 'msrc']);
const RESEARCH_PROFILE_IDS = new Set(['security-research', 'mathematics']);
const OPTIONAL_MODELS = new Map([
  ['openai-codex\u0000gpt-daybreak-blue-latest', true],
  ['openai-codex\u0000gpt-daybreak-red-latest', false],
  ['anthropic\u0000claude-fable-5', true],
  ['anthropic\u0000claude-mythos-5', false],
]);

export function isResearchKitId(value: unknown): boolean {
  return typeof value === 'string' && RESEARCH_KIT_IDS.has(value);
}

export function isResearchProfileId(value: unknown): boolean {
  return typeof value === 'string' && RESEARCH_PROFILE_IDS.has(value);
}

export function isWorkspaceMemoryBackendId(value: unknown): boolean {
  return value === 'app-server' || value === 'disabled';
}

export function isOptionalProviderModel(providerId: string, modelId: string): boolean {
  return OPTIONAL_MODELS.has(`${providerId}\u0000${modelId}`);
}

export function isOptionalProviderModelEnabled(
  settings: { enabledOptionalModels?: Record<string, string[]>; disabledOptionalModels?: Record<string, string[]> } | null,
  providerId: string,
  modelId: string,
): boolean {
  if (settings?.disabledOptionalModels?.[providerId]?.includes(modelId)) return false;
  if (settings?.enabledOptionalModels?.[providerId]?.includes(modelId)) return true;
  return OPTIONAL_MODELS.get(`${providerId}\u0000${modelId}`) === true;
}

export function readWorkspaceDescription(workspacePath: string): string {
  try {
    return readFileSync(join(workspacePath, 'AGENTS.md'), 'utf8');
  } catch {
    return '';
  }
}
