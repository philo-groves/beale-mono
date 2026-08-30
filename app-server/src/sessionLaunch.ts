import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import type {
  HoneycrispProviderAuthenticationMethod,
  HoneycrispProviderRiskAcknowledgement
} from 'honeycrisp/protocol';
import type { AppServerMemoryBackendId } from './hostRegistry.js';

const DEFAULT_TOOL_MAX_BYTES = 200_000;
const PROFILE_TOOL_FAMILY_CEILING_DEFAULT = ['shell', 'repository-search', 'file-read'] as const;
const PROFILE_TOOL_FAMILY_CEILING_ALLOWED = new Set([
  'shell',
  'repository-search',
  'file-read',
  'code',
  'analysis',
  'synthesis',
  'storage',
  'experiment'
]);
const PROFILE_SIDE_EFFECT_CEILING_DEFAULT = ['none', 'read', 'write', 'process'] as const;
const PROFILE_SIDE_EFFECT_CEILING_ALLOWED = new Set(PROFILE_SIDE_EFFECT_CEILING_DEFAULT);

const RISK_ACKNOWLEDGEMENT_FLAGS: Record<HoneycrispProviderRiskAcknowledgement, string> = {
  'openai-codex': '--openai-trusted-access-cyber-risk-acknowledged',
  anthropic: '--anthropic-cvp-risk-acknowledged',
  xai: '--xai-policy-risk-acknowledged',
  zai: '--zai-policy-risk-acknowledged',
  openrouter: '--openrouter-policy-risk-acknowledged'
};

export interface PreparedHoneycrispSessionLaunch {
  args: string[];
  env: NodeJS.ProcessEnv;
}

export interface ResolvedHoneycrispSessionLaunch {
  workspaceRoot: string;
  workspaceDirectories: readonly string[];
  capturePath: string;
  workspaceContextPath?: string;
  attemptId: string;
  promptMarkdown: string;
  goal?: { objective?: string };
  provider: {
    id: string;
    model?: string;
    reasoningEffort?: string;
    fastMode?: boolean;
    riskAcknowledgements: readonly HoneycrispProviderRiskAcknowledgement[];
    authenticationPreferences: Readonly<Record<string, HoneycrispProviderAuthenticationMethod>>;
    title?: { model?: string; effort: string };
    shellReview?: { models: Readonly<Record<string, string>>; effort: string };
  };
  shellSafetyMode: string;
  shellOptionsPath?: string;
  collaborationConfigPath?: string;
  resumeCapturePath?: string;
  resumeFallbackPromptPath?: string;
  workflowId?: string;
  researchProfileId?: string;
  researchProfileHash?: string;
  profileAware: boolean;
  memoryBackend: AppServerMemoryBackendId;
  pluginRuntime?: {
    skillDirectories?: readonly string[];
    selectedSkillIds?: readonly string[];
    mcpConfigPath?: string;
    allowedMcpServers?: readonly string[];
  };
  introspection?: {
    url: string;
    token: string;
  };
  memoryTypeDescriptions?: Readonly<Record<string, string>>;
  storage: { databasePath: string; artifactDirectoryPath: string };
}

/**
 * The sole mapping from the versioned app-server launch DTO to Honeycrisp's
 * hosted runtime arguments and worker environment. Mobile and desktop clients never construct
 * transport or runtime-policy flags themselves.
 */
export function prepareHoneycrispSessionLaunch(
  launch: ResolvedHoneycrispSessionLaunch,
  environment: NodeJS.ProcessEnv = process.env
): PreparedHoneycrispSessionLaunch {
  return {
    args: honeycrispSessionArgs(launch, environment),
    env: honeycrispSessionEnvironment(launch, environment)
  };
}

export function honeycrispSessionArgs(
  launch: ResolvedHoneycrispSessionLaunch,
  environment: NodeJS.ProcessEnv = process.env
): string[] {
  const provider = environment.BEALE_HONEYCRISP_PROVIDER?.trim() || launch.provider.id.trim();
  if (!provider) throw new Error('provider.id must be non-empty.');

  const args = [
    '--workspace-root',
    launch.workspaceRoot,
    '--capture',
    launch.capturePath,
    '--executor',
    'agent',
    '--attempt-id',
    launch.attemptId,
    ...launch.workspaceDirectories.flatMap((directory) => ['--repo-root', directory]),
    '-p',
    launch.promptMarkdown
  ];

  if (launch.workspaceContextPath) args.push('--workspace-context', launch.workspaceContextPath);

  if (launch.resumeCapturePath) args.push('--resume-capture', launch.resumeCapturePath);
  if (launch.resumeFallbackPromptPath) {
    args.push('--resume-fallback-prompt-file', launch.resumeFallbackPromptPath);
  }
  if (launch.goal) {
    args.push('--goal');
    if (launch.goal.objective) args.push('--goal-objective', launch.goal.objective);
  }
  if (enabled(environment.BEALE_HONEYCRISP_MOCK)) args.push('--mock');
  const configPath = environment.BEALE_HONEYCRISP_CONFIG?.trim();
  if (configPath) args.push('--config', configPath);

  args.push('--provider', provider);
  if (launch.collaborationConfigPath) {
    args.push('--collaboration-config', launch.collaborationConfigPath);
  }
  for (const acknowledgement of launch.provider.riskAcknowledgements ?? []) {
    args.push(RISK_ACKNOWLEDGEMENT_FLAGS[acknowledgement]);
  }
  if (launch.provider.title) {
    if (launch.provider.title.model) {
      args.push('--title-model-default', launch.provider.title.model);
    }
    args.push('--title-effort-default', launch.provider.title.effort);
  }
  if (launch.provider.model) args.push('--model', launch.provider.model);
  if (launch.provider.reasoningEffort) args.push('--effort', launch.provider.reasoningEffort);
  if (launch.provider.fastMode) args.push('--fast-mode');

  if (launch.researchProfileId) args.push('--research-profile-id', launch.researchProfileId);
  if (launch.researchProfileHash) args.push('--research-profile-hash', launch.researchProfileHash);
  if (launch.workflowId) args.push('--workflow', launch.workflowId);
  args.push('--memory-backend', launch.memoryBackend);

  args.push(...additionalRuntimeArgs(environment));
  args.push('--no-default-tool-config');
  appendPluginRuntimeArgs(args, launch);
  args.push(...capabilityArgs(launch.profileAware, environment));
  args.push('--allowed-side-effect', 'network');
  if (launch.shellOptionsPath) args.push('--shell-options', launch.shellOptionsPath);

  // Host-controlled reviewer routing follows extension arguments so operator
  // runtime flags cannot replace the authorization model selection.
  args.push('--shell-safety-mode', launch.shellSafetyMode);
  if (launch.provider.shellReview) {
    args.push('--shell-review-models', JSON.stringify(launch.provider.shellReview.models));
    args.push('--shell-review-effort', launch.provider.shellReview.effort);
  }
  if (!launch.profileAware && launch.memoryTypeDescriptions) {
    args.push('--memory-type-descriptions', JSON.stringify(launch.memoryTypeDescriptions));
  }
  args.push('--tool-max-bytes', String(positiveInteger(environment.BEALE_HONEYCRISP_TOOL_MAX_BYTES) ?? DEFAULT_TOOL_MAX_BYTES));
  return args;
}

export function honeycrispSessionEnvironment(
  launch: ResolvedHoneycrispSessionLaunch,
  environment: NodeJS.ProcessEnv = process.env
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {
    ...environment,
    NO_COLOR: environment.NO_COLOR ?? '1',
    HONEYCRISP_DATABASE_PATH: launch.storage.databasePath,
    HONEYCRISP_ARTIFACT_DIRECTORY: launch.storage.artifactDirectoryPath,
    HONEYCRISP_PROVIDER_AUTH_PREFERENCES: JSON.stringify({
      'openai-codex': launch.provider.authenticationPreferences?.['openai-codex'] ?? 'subscription',
      anthropic: launch.provider.authenticationPreferences?.anthropic ?? 'subscription',
      xai: launch.provider.authenticationPreferences?.xai ?? 'subscription',
      zai: launch.provider.authenticationPreferences?.zai ?? 'subscription',
      openrouter: launch.provider.authenticationPreferences?.openrouter ?? 'api_key'
    })
  };
  if (launch.introspection) {
    env.BEALE_INTROSPECTION_URL = launch.introspection.url;
    env.BEALE_INTROSPECTION_TOKEN = launch.introspection.token;
  }
  if (!env.HONEYCRISP_CODEX_AUTH_FILE?.trim()) {
    const codexAuthFile = resolveHoneycrispCodexAuthFile(environment);
    if (codexAuthFile) env.HONEYCRISP_CODEX_AUTH_FILE = codexAuthFile;
  }
  return env;
}

export function resolveHoneycrispCodexAuthFile(
  environment: NodeJS.ProcessEnv = process.env,
  userHome = homedir()
): string | undefined {
  const honeycrispPath = environment.HONEYCRISP_CODEX_AUTH_FILE?.trim();
  if (honeycrispPath) return resolve(honeycrispPath.replace(/^~(?=$|[\\/])/, userHome));
  const configured = environment.BEALE_OPENAI_CODEX_AUTH_FILE?.trim();
  const candidate = resolve(configured
    ? configured.replace(/^~(?=$|[\\/])/, userHome)
    : join(userHome, '.codex', 'auth.json'));
  return existsSync(candidate) ? candidate : undefined;
}

function appendPluginRuntimeArgs(args: string[], launch: ResolvedHoneycrispSessionLaunch): void {
  const runtime = launch.pluginRuntime;
  if (!runtime) return;
  for (const path of runtime.skillDirectories ?? []) args.push('--skill-dir', path);
  for (const skillId of runtime.selectedSkillIds ?? []) args.push('--skill', skillId);
  if (runtime.mcpConfigPath) args.push('--mcp-config', runtime.mcpConfigPath);
  for (const serverName of runtime.allowedMcpServers ?? []) {
    args.push('--allow-mcp-server', serverName);
  }
}

function capabilityArgs(profileAware: boolean, environment: NodeJS.ProcessEnv): string[] {
  if (profileAware) {
    const toolFamilies = parseCapabilityCeiling(
      environment.BEALE_HONEYCRISP_PROFILE_TOOL_FAMILY_CEILING_JSON,
      'BEALE_HONEYCRISP_PROFILE_TOOL_FAMILY_CEILING_JSON',
      PROFILE_TOOL_FAMILY_CEILING_DEFAULT,
      PROFILE_TOOL_FAMILY_CEILING_ALLOWED
    );
    const sideEffects = parseCapabilityCeiling(
      environment.BEALE_HONEYCRISP_PROFILE_SIDE_EFFECT_CEILING_JSON,
      'BEALE_HONEYCRISP_PROFILE_SIDE_EFFECT_CEILING_JSON',
      PROFILE_SIDE_EFFECT_CEILING_DEFAULT,
      PROFILE_SIDE_EFFECT_CEILING_ALLOWED
    );
    return [
      ...toolFamilies.flatMap((family) => ['--profile-tool-family-ceiling', family]),
      ...sideEffects.flatMap((effect) => ['--profile-side-effect-ceiling', effect])
    ];
  }
  return [
    '--tool-family',
    'shell',
    '--allowed-side-effect',
    'read',
    '--allowed-side-effect',
    'write',
    '--allowed-side-effect',
    'process',
    '--disable-tool-family',
    'repository-search',
    '--disable-tool-family',
    'file-read',
    '--disable-tool-family',
    'code',
    '--disable-tool-family',
    'analysis',
    '--disable-tool-family',
    'synthesis',
    '--disable-tool-family',
    'storage',
    '--disable-tool-family',
    'experiment'
  ];
}

function parseCapabilityCeiling(
  raw: string | undefined,
  name: string,
  fallback: readonly string[],
  allowed: ReadonlySet<string>
): string[] {
  if (!raw?.trim()) return [...fallback];
  const parsed = JSON.parse(raw) as unknown;
  if (!Array.isArray(parsed) || !parsed.every((item) => typeof item === 'string')) {
    throw new Error(`${name} must be a JSON string array.`);
  }
  const normalized = [...new Set(parsed.map((item) => item.trim()))];
  const invalid = normalized.find((item) => !item || !allowed.has(item));
  if (invalid !== undefined) {
    throw new Error(`${name} contains an unsupported capability: ${invalid || '[empty]'}.`);
  }
  return normalized;
}

function additionalRuntimeArgs(environment: NodeJS.ProcessEnv): string[] {
  const raw = environment.BEALE_HONEYCRISP_RUNTIME_ARGS_JSON?.trim();
  if (!raw) return [];
  const parsed = JSON.parse(raw) as unknown;
  if (!Array.isArray(parsed) || parsed.some((value) => typeof value !== 'string' || !value)) {
    throw new Error('BEALE_HONEYCRISP_RUNTIME_ARGS_JSON must be a JSON array of non-empty strings.');
  }
  return parsed as string[];
}

function positiveInteger(value: string | undefined): number | null {
  if (!value?.trim()) return null;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function enabled(value: string | undefined): boolean {
  return value === '1' || value === 'true';
}
