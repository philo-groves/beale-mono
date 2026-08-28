import { readFile } from 'node:fs/promises';

const NON_RECOVERABLE_FAILURE_PATTERNS = [
  /\b(?:authentication|credentials?|api key|oauth|login)\b.{0,80}\b(?:failed|missing|required|expired|invalid|unavailable)\b/iu,
  /\b(?:unauthorized|forbidden|permission denied|eacces)\b/iu,
  /\b(?:safety|cyber) (?:guardrail|safeguard)\b/iu,
  /\b(?:content|provider) policy\b/iu,
  /\bmanual approval\b.{0,80}\b(?:denied|required|unavailable)\b/iu,
  /\bauthorization\b.{0,80}\b(?:not recorded|required|failed|outside)\b/iu,
  /\boutside (?:the )?(?:recorded |authorized )?scope\b/iu,
  /\b(?:database|session) integrity\b/iu,
  /\b(?:database|sqlite)\b.{0,80}\b(?:corrupt|malformed)\b/iu,
  /\b(?:invalid|unsupported|unknown)\b.{0,80}\b(?:provider|model|profile|workflow|configuration|schema|capability)\b/iu,
  /\b(?:profile|workspace|session)\b.{0,80}\b(?:hash mismatch|does not belong|failed validation)\b/iu,
  /\b(?:enoent|not found)\b.{0,80}\b(?:workspace|context|config|profile)\b/iu,
] as const;

export const DEFAULT_LONG_SESSION_RECOVERY_ATTEMPTS = 2;

export interface HoneycrispSessionCompletion {
  readonly succeeded: boolean;
  readonly recoverable: boolean;
  readonly captureAvailable: boolean;
  readonly diagnostic: string | null;
}

export async function inspectHoneycrispSessionCompletion(input: {
  code: number | null;
  stderr: string;
  capturePath: string;
  stopRequested: boolean;
}): Promise<HoneycrispSessionCompletion> {
  if (input.stopRequested) {
    return {
      succeeded: false,
      recoverable: false,
      captureAvailable: false,
      diagnostic: null,
    };
  }

  const capture = await readCaptureResult(input.capturePath);
  const succeeded = capture?.status === 'complete'
    || (capture === null && input.code === 0);
  if (succeeded) {
    return {
      succeeded: true,
      recoverable: false,
      captureAvailable: capture !== null,
      diagnostic: null,
    };
  }

  const diagnostic = normalizedDiagnostic(
    capture?.status === 'error' ? capture.outputText : input.stderr,
  ) ?? `Honeycrisp exited with code ${input.code ?? 'unknown'}.`;
  return {
    succeeded: false,
    recoverable: isRecoverableLongSessionFailure(diagnostic),
    captureAvailable: capture !== null,
    diagnostic,
  };
}

export function isRecoverableLongSessionFailure(message: string): boolean {
  const normalized = message.trim();
  if (!normalized) return true;
  return !NON_RECOVERABLE_FAILURE_PATTERNS.some((pattern) => pattern.test(normalized));
}

export function longSessionRecoveryDelayMs(recoveryNumber: number): number {
  if (!Number.isFinite(recoveryNumber) || recoveryNumber <= 1) return 0;
  return Math.min((Math.floor(recoveryNumber) - 1) * 2_000, 10_000);
}

export function longSessionRecoveryFallbackPrompt(
  originalPrompt: string,
  diagnostic: string,
): string {
  return [
    '# Recover the existing Beale research session',
    '',
    'The previous worker ended unexpectedly. Continue the same research task from the durable session state and prior attempt capture. Preserve established evidence, decisions, open hypotheses, and completed work. Do not restart the investigation or repeat prior work merely to reconstruct context.',
    '',
    'If native provider state is unavailable, inspect the canonical session history, workspace memory, research resources, and campaign state before choosing the next action.',
    '',
    '## Original request',
    originalPrompt.trim(),
    '',
    '## Recovery note',
    diagnostic.trim().slice(0, 2_000),
  ].join('\n');
}

async function readCaptureResult(
  capturePath: string,
): Promise<{ status: 'complete' | 'error'; outputText: string } | null> {
  try {
    const parsed = JSON.parse(await readFile(capturePath, 'utf8')) as unknown;
    if (!isRecord(parsed) || !isRecord(parsed.agent)) return null;
    const status = parsed.agent.status;
    if (status !== 'complete' && status !== 'error') return null;
    return {
      status,
      outputText: typeof parsed.agent.outputText === 'string' ? parsed.agent.outputText : '',
    };
  } catch {
    return null;
  }
}

function normalizedDiagnostic(value: string): string | null {
  const normalized = value.trim();
  return normalized || null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}
