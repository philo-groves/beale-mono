import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { JSX, KeyboardEvent as ReactKeyboardEvent } from 'react';
import type { BealeWorkspaceResearchTrack, BealeWorkspaceResearchTrackRequest } from '@beale/app-server-runtime/protocol';
import { ArrowRight, Check, ChevronDown, ChevronRight, Compass, FileText, Lightbulb, Monitor, Play, Plus, RefreshCw, Repeat, Settings, ShieldAlert, Sparkles, Telescope, Waypoints, X } from 'lucide-react';
import type {
  HostEnvironment,
  OpenAiAccountStatus,
  ResearchGoalPhase,
  ResearchGoalSuggestionsByPhase,
  ResearchGoalSuggestionStateByPhase,
  ResearchModelEffortLevel,
  ResearchModelProviderId,
  ResearchModelSelection,
  ProviderModelDefaults,
  ProviderSettings,
  ResearchCollaborationProviderPreference,
  ResearchSubagentMode,
  ResearchProfileSnapshot,
  ResearchProfileWorkflow,
  ResearchProviderModel,
  ResearchProviderModelCatalog,
  ResearchProviderStatus,
  ResearchPromptGenerationInput,
  GeneratedResearchPrompt,
  RepeatSchedule,
  RunRecord,
  ShellSafetyMode,
  StartRunInput,
  FleetState,
  WorkspaceSnapshot
} from '@shared/types';
import { resolveGoalObjective } from '../../../shared/goalObjective';
import { allowedFleetTargets, defaultFleetMachineId, isAllowedFleetMachine, type FleetTargetChoice } from '../../../shared/fleet';
import { ensureDefaultResearchCollaborator, normalizeResearchCollaboration } from '../../../shared/collaboration';
import { Modal } from '../../app/Modal';
import { BealeWelcomeIcon } from '../../app/BealeWelcomeIcon';
import { MainSideScrollRegion } from '../../app/MainSideScrollRegion';
import { ModelSelectionPicker } from '../../app/ModelSelectionPicker';
import { userFacingErrorMessage } from '../../lib/errors';
import { researchModelNameLabel } from '../../lib/formatting';
import { DEFAULT_RESEARCH_MODEL } from '../../../shared/modelDefaults';
import { normalizeRepeatSchedule, repeatScheduleFor, repeatScheduleLabel } from '../../../shared/repeatSchedule';
import { DEFAULT_SHELL_SAFETY_MODE } from '../../../shared/shellSafety';
import {
  clientRequestId,
  defaultRunInput
} from '../../view-models/runSettings';
import type { ResearchGoalSeed } from './SessionNextSteps';
import { CommentaryView } from '../commentary/CommentaryView';
import { SessionNextStepsWidget } from './SessionNextSteps';

const PROMPT_STREAM_RENDER_INTERVAL_MS = 90;
const MAX_RENDERED_GOAL_SUGGESTIONS = 12;
const FLEET_READ_TIMEOUT_MS = 20_000;

export async function loadWorkspaceFleetStateWithRetry(
  workspaceId: string,
  getState: (workspaceId: string) => Promise<FleetState>,
  waitToRetry: () => Promise<void> = () => new Promise((resolve) => setTimeout(resolve, 500)),
  timeoutMs = FLEET_READ_TIMEOUT_MS
): Promise<FleetState> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        getState(workspaceId),
        new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => reject(new Error('Target loading timed out. Retry to check this workspace again.')), timeoutMs);
        })
      ]);
    } catch (error) {
      if (attempt === 1) throw error;
      await waitToRetry();
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  }
  throw new Error('Target loading failed.');
}

export function newResearchPromptPlaceholder(addContext: boolean): string {
  return addContext ? 'Write a sentence or two' : 'Write a full research prompt';
}

export function researchTrackConfiguration(input: StartRunInput,
  guardianEnabled = true): NonNullable<BealeWorkspaceResearchTrackRequest['configuration']> {
  if (!input.provider || !input.model || !input.targetIds?.length) {
    throw new Error('Choose a lead model and at least one target before starting the track.');
  }
  const collaboration = input.collaboration ? normalizeResearchCollaboration(input.collaboration) : null;
  return { provider: { id: input.provider, model: input.model, reasoningEffort: input.reasoningEffort || 'off' },
    guardianEnabled,
    targetIds: input.targetIds,
    ...(collaboration ? { collaboration: {
      ...collaboration,
      providers: collaboration.providers.filter((preference) => preference.enabled)
    } } : {}) };
}

export function enableCollaboratorAtTop(
  providers: ResearchCollaborationProviderPreference[],
  collaborator: ResearchCollaborationProviderPreference | null
): ResearchCollaborationProviderPreference[] {
  if (!collaborator) return providers;
  return [
    { ...collaborator, enabled: true },
    ...providers.filter((provider) => (
      provider.provider !== collaborator.provider || provider.model !== collaborator.model
    ))
  ];
}
const REPEAT_SCHEDULE_TYPES: RepeatSchedule['type'][] = ['none', 'minutely', 'hourly', 'daily', 'weekly', 'monthly'];
type RepeatScheduleUnit = 'minute' | 'hour' | 'day' | 'week' | 'month';

type PromptEditorStage = 'goal' | 'prompt';

interface SessionProviderOption {
  id: ResearchModelProviderId;
  label: string;
  configured: boolean;
  models: ResearchProviderModel[];
}

interface ResearchGoalChooserProps {
  workflows?: readonly ResearchProfileWorkflow[];
  suggestions: ResearchGoalSuggestionsByPhase;
  loading: ResearchGoalSuggestionStateByPhase<boolean>;
  errors: ResearchGoalSuggestionStateByPhase<string | null>;
  selectedWorkflowId?: ResearchGoalPhase;
  onSelectWorkflow?: (phase: ResearchGoalPhase) => void;
  onLoad?: (phase: ResearchGoalPhase) => void;
  onSelect: (sentence: string, phase: ResearchGoalPhase) => void;
  onRetry: (phase: ResearchGoalPhase) => void;
}

const LEGACY_RESEARCH_GOAL_WORKFLOWS: readonly ResearchProfileWorkflow[] = [
  {
    id: 'discovery',
    name: 'Discovery',
    description: 'Find a new primitive by pairing a system area with a plausible bug class; reachability, exploitability, and reportability remain open.',
    goalSuggestionCount: 4,
    goalSuggestionInstructions: [],
    promptInstructions: [],
    outputRequirements: [],
    default: true
  },
  {
    id: 'chaining',
    name: 'Chaining',
    description: 'Upgrade existing primitives into a reportable exploit chain and triage-ready PoC, discovering missing links when needed.',
    goalSuggestionCount: 4,
    goalSuggestionInstructions: [],
    promptInstructions: [],
    outputRequirements: []
  },
  {
    id: 'reporting',
    name: 'Reporting',
    description: 'Document a supported chain, its bugs and impact, and package the triage-ready PoC and required evidence in submission.zip.',
    goalSuggestionCount: 4,
    goalSuggestionInstructions: [],
    promptInstructions: [],
    outputRequirements: []
  },
  {
    id: 'longshot',
    name: 'Longshot',
    description: 'Hunt for ambitious, reportable high- or critical-severity vulnerabilities.',
    goalSuggestionCount: 4,
    goalSuggestionInstructions: [],
    promptInstructions: [],
    outputRequirements: []
  }
];

interface StartRunFormProps {
  snapshot: WorkspaceSnapshot;
  appServerName?: string;
  initialInput?: StartRunInput;
  autoOpenSchedule?: boolean;
  openAiStatus: OpenAiAccountStatus | null;
  defaultProviderId: ResearchModelProviderId | null | undefined;
  defaultShellSafetyMode?: ShellSafetyMode;
  guardianEnabled?: boolean;
  providerModelDefaults: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>> | undefined;
  providerPolicyRiskAcknowledgements?: ProviderSettings['cyberPolicyRiskAcknowledgements'];
  researchProviderStatuses: ResearchProviderStatus[];
  providerModelCatalog: ResearchProviderModelCatalog[];
  researchGoalSuggestions: ResearchGoalSuggestionsByPhase;
  researchGoalSuggestionsLoading: ResearchGoalSuggestionStateByPhase<boolean>;
  researchGoalSuggestionErrors: ResearchGoalSuggestionStateByPhase<string | null>;
  initialGoal?: ResearchGoalSeed | null;
  showSuggestions?: boolean;
  presentation?: 'dialog' | 'session';
  busy: boolean;
  runAction: (action: () => Promise<WorkspaceSnapshot | null | void>, options?: { refreshSnapshot?: boolean }) => Promise<void>;
  onCancel: () => void;
  onLoadResearchGoalSuggestions?: (phase: ResearchGoalPhase) => void;
  onSelectResearchGoalSuggestion?: (phase: ResearchGoalPhase, suggestion: string) => void;
  onRetryResearchGoalSuggestions: (phase: ResearchGoalPhase) => void;
  onStarted: (run: RunRecord) => void;
  trackMode?: boolean;
  onTrackStarted?: (track: BealeWorkspaceResearchTrack) => void;
}

export interface ResearchSettingsFormProps {
  researchProfile: ResearchProfileSnapshot | null;
  formIdentity: string;
  fleetWorkspaceId?: string;
  remoteTargetChoices?: FleetTargetChoice[];
  configuredProviderIds?: readonly ResearchModelProviderId[];
  appServerName?: string;
  autoOpenSchedule?: boolean;
  workspaceName?: string;
  openAiStatus: OpenAiAccountStatus | null;
  defaultProviderId: ResearchModelProviderId | null | undefined;
  defaultShellSafetyMode?: ShellSafetyMode;
  providerModelDefaults: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>> | undefined;
  providerPolicyRiskAcknowledgements?: ProviderSettings['cyberPolicyRiskAcknowledgements'];
  requireCyberPolicyAcknowledgement?: boolean;
  researchProviderStatuses: ResearchProviderStatus[];
  providerModelCatalog: ResearchProviderModelCatalog[];
  researchGoalSuggestions?: ResearchGoalSuggestionsByPhase;
  researchGoalSuggestionsLoading?: ResearchGoalSuggestionStateByPhase<boolean>;
  researchGoalSuggestionErrors?: ResearchGoalSuggestionStateByPhase<string | null>;
  initialGoal?: ResearchGoalSeed | null;
  initialInput?: StartRunInput;
  showSuggestions?: boolean;
  showAddContext?: boolean;
  trackMode?: boolean;
  generatePrompt?: (input: ResearchPromptGenerationInput) => Promise<GeneratedResearchPrompt>;
  disableNoRepeat?: boolean;
  presentation?: 'dialog' | 'embedded' | 'session';
  title?: string;
  submitLabel?: string;
  busy: boolean;
  onCancel?: () => void;
  onLoadResearchGoalSuggestions?: (phase: ResearchGoalPhase) => void;
  onSelectResearchGoalSuggestion?: (phase: ResearchGoalPhase, suggestion: string) => void;
  onRetryResearchGoalSuggestions?: (phase: ResearchGoalPhase) => void;
  onSubmit: (input: StartRunInput) => Promise<void> | void;
}

export function StartRunForm(props: StartRunFormProps): JSX.Element {
  const {
    snapshot,
    runAction,
    onStarted,
    presentation = 'dialog',
    ...settingsProps
  } = props;
  const [credentialAccess, setCredentialAccess] = useState<{
    input: StartRunInput;
    providerIds: ResearchModelProviderId[];
  } | null>(null);
  const [credentialAccessBusy, setCredentialAccessBusy] = useState(false);
  const [credentialAccessError, setCredentialAccessError] = useState<string | null>(null);
  const preparingRun = useRef(false);
  const launchRun = async (input: StartRunInput): Promise<void> => {
    if (props.trackMode) {
      let startedTrack: BealeWorkspaceResearchTrack | null = null;
      await runAction(async () => {
        startedTrack = await window.beale.controlResearchTrack(snapshot.workspace.workspaceId, 'start', undefined,
          researchTrackConfiguration(input, props.guardianEnabled !== false));
      }, { refreshSnapshot: false });
      if (startedTrack) props.onTrackStarted?.(startedTrack);
      return;
    }
    let latestRun: RunRecord | undefined;
    await runAction(async () => {
      const next = await window.beale.startRun({ ...input, guardianEnabled: props.guardianEnabled !== false });
      latestRun = next.runs[0]?.run;
      return next;
    });
    if (latestRun) onStarted(latestRun);
  };
  const prepareCredentials = async (input: StartRunInput): Promise<void> => {
    try {
      const access = await window.beale.getProviderCredentialAccessRequest(selectedSessionProviderIds(input));
      if (access.providerIds.length > 0) {
        setCredentialAccess({ input, providerIds: access.providerIds });
        setCredentialAccessError(null);
        return;
      }
      await launchRun(input);
    } catch (caught) {
      await runAction(async () => { throw caught; });
    }
  };
  const prepareRun = async (input: StartRunInput): Promise<void> => {
    if (preparingRun.current || credentialAccess) return;
    preparingRun.current = true;
    try {
      const selectedTargetId = input.targetIds?.[0] ?? input.machineId;
      if (selectedTargetId && !props.trackMode) {
        await window.beale.configureFleet({ action: 'select-machine', workspaceId: snapshot.workspace.workspaceId, machineId: selectedTargetId });
      }
      await prepareCredentials(input);
    } catch (caught) {
      await runAction(async () => { throw caught; });
    } finally {
      preparingRun.current = false;
    }
  };
  const continueWithCredentialAccess = async (): Promise<void> => {
    if (!credentialAccess || credentialAccessBusy) return;
    setCredentialAccessBusy(true);
    setCredentialAccessError(null);
    try {
      await window.beale.unlockProviderApiKeys(credentialAccess.providerIds);
      const input = credentialAccess.input;
      setCredentialAccess(null);
      await launchRun(input);
    } catch (caught) {
      setCredentialAccessError(userFacingErrorMessage(caught));
    } finally {
      setCredentialAccessBusy(false);
    }
  };
  const credentialProviderNames = credentialAccess?.providerIds.map((providerId) => {
    const catalog = props.providerModelCatalog.find((candidate) => candidate.providerId === providerId);
    return providerLabel(providerId, catalog?.providerName ?? providerId);
  }) ?? [];
  return (
    <>
      <ResearchSettingsForm
        {...settingsProps}
        busy={props.busy}
        researchProfile={snapshot.researchProfile ?? null}
        fleetWorkspaceId={snapshot.workspace.workspaceId}
        formIdentity={`${snapshot.workspace.workspaceId}:${snapshot.activeScope.id}:${snapshot.researchProfile?.profileHash ?? 'default'}`}
        workspaceName={snapshot.activeScope.workspaceName ?? 'Workspace'}
        showSuggestions={props.showSuggestions ?? true}
        presentation={presentation}
        trackMode={props.trackMode}
        title={props.trackMode ? 'Start Track' : undefined}
        showAddContext={!props.trackMode}
        onSubmit={prepareRun}
      />
      {credentialAccess ? (
        <ProviderKeychainAccessDialog
          busy={credentialAccessBusy}
          error={credentialAccessError}
          platform={snapshot.workspace.hostEnvironment.platform}
          providerNames={credentialProviderNames}
          onCancel={() => setCredentialAccess(null)}
          onContinue={() => void continueWithCredentialAccess()}
        />
      ) : null}
    </>
  );
}

export function ProviderKeychainAccessDialog({
  busy,
  error,
  platform,
  providerNames,
  onCancel,
  onContinue
}: {
  busy: boolean;
  error: string | null;
  platform: HostEnvironment['platform'];
  providerNames: readonly string[];
  onCancel: () => void;
  onContinue: () => void;
}): JSX.Element {
  return (
    <Modal
      className="provider-keychain-access-dialog"
      closeDisabled={busy}
      footer={(
        <>
          <button className="secondary-button" disabled={busy} type="button" onClick={onCancel}>Cancel</button>
          <button className="primary-button" disabled={busy} type="button" onClick={onContinue}>
            {busy ? 'Accessing…' : 'Continue'}
          </button>
        </>
      )}
      onClose={onCancel}
      title="Access Saved API Key"
    >
      <div className="provider-keychain-access-message">
        <p>Beale needs to read the saved API {providerNames.length === 1 ? 'key' : 'keys'} for {formatProviderNames(providerNames)} from the operating system&apos;s secure storage to start this session.</p>
        {platform === 'darwin' ? <p>After you continue, macOS may show a “Beale Safe Storage” password prompt.</p> : null}
        {error ? <p className="provider-keychain-access-error" role="alert">{error}</p> : null}
      </div>
    </Modal>
  );
}

export function ResearchSettingsForm({
  researchProfile,
  formIdentity,
  fleetWorkspaceId,
  remoteTargetChoices,
  configuredProviderIds,
  appServerName = 'Local',
  autoOpenSchedule = false,
  workspaceName = 'Workspace',
  openAiStatus,
  defaultProviderId,
  defaultShellSafetyMode = DEFAULT_SHELL_SAFETY_MODE,
  providerModelDefaults,
  providerPolicyRiskAcknowledgements = undefined,
  requireCyberPolicyAcknowledgement = false,
  researchProviderStatuses,
  providerModelCatalog,
  researchGoalSuggestions = {},
  researchGoalSuggestionsLoading = {},
  researchGoalSuggestionErrors = {},
  initialGoal = null,
  initialInput,
  showSuggestions = true,
  showAddContext = true,
  trackMode = false,
  generatePrompt,
  disableNoRepeat = false,
  presentation: formPresentation = 'embedded',
  title,
  submitLabel = 'Save changes',
  busy,
  onCancel = () => undefined,
  onLoadResearchGoalSuggestions = () => undefined,
  onSelectResearchGoalSuggestion = () => undefined,
  onRetryResearchGoalSuggestions = () => undefined,
  onSubmit
}: ResearchSettingsFormProps): JSX.Element {
  const profile = researchProfile?.profile;
  const workflows = profile?.workflows.length ? profile.workflows : LEGACY_RESEARCH_GOAL_WORKFLOWS;
  const defaultWorkflowId = defaultResearchWorkflowId(workflows);
  const profilePresentation = profile?.presentation;
  const initialWorkflowId = workflows.some((workflow) => workflow.id === initialGoal?.phase)
    ? initialGoal!.phase
    : defaultWorkflowId;
  const [input, setInput] = useState<StartRunInput>(() => (
    researchSettingsInput(initialInput, initialWorkflowId, initialGoal, defaultShellSafetyMode)
  ));
  const [selectedMachineIds, setSelectedMachineIds] = useState<string[]>(initialInput?.targetIds?.length
    ? initialInput.targetIds : initialInput?.machineId ? [initialInput.machineId]
      : remoteTargetChoices?.find((target) => target.ready) ? [remoteTargetChoices.find((target) => target.ready)!.id] : []);
  const selectedMachineIdsRef = useRef(selectedMachineIds);
  const [fleetState, setFleetState] = useState<FleetState | null>(null);
  const [fleetError, setFleetError] = useState<string | null>(null);
  const [fleetLoadAttempt, setFleetLoadAttempt] = useState(0);
  const [startingRun, setStartingRun] = useState(false);
  const [editorStage, setEditorStage] = useState<PromptEditorStage>(initialInput || initialGoal?.promptMarkdown ? 'prompt' : 'goal');
  const [generateEnabled, setGenerateEnabled] = useState(false);
  const [generatingPrompt, setGeneratingPrompt] = useState(false);
  const [generationError, setGenerationError] = useState<string | null>(null);
  const [selectedProviderId, setSelectedProviderId] = useState<ResearchModelProviderId | null>(
    researchProviderId(initialInput?.provider) ?? defaultProviderId ?? null
  );
  const providerSelectionInitializedRef = useRef(Boolean(initialInput?.provider));
  const modelSelectionInitializedRef = useRef(Boolean(initialInput?.model));
  const promptBoxRef = useRef<HTMLTextAreaElement | null>(null);
  const inputRef = useRef(input);
  const mountedRef = useRef(true);
  const generationRequestIdRef = useRef<string | null>(null);
  const generationSourceTextRef = useRef<string | null>(null);
  const pendingPromptMarkdownRef = useRef<string | null>(null);
  const promptStreamFlushTimerRef = useRef<number | null>(null);
  const promptStreamAutoScrollRef = useRef(false);
  const providerOptions = useMemo<SessionProviderOption[]>(
    () => providerModelCatalog.map((catalog) => ({
      id: catalog.providerId,
      label: providerLabel(catalog.providerId, catalog.providerName),
      configured: configuredProviderIds
        ? configuredProviderIds.includes(catalog.providerId)
        : catalog.providerId.startsWith('custom-')
          ? true
        : catalog.providerId === 'openai-codex'
          ? openAiStatus?.configured ?? false
          : researchProviderStatuses.find((provider) => provider.id === catalog.providerId)?.configured ?? false,
      models: catalog.models
    })),
    [configuredProviderIds, openAiStatus, providerModelCatalog, researchProviderStatuses]
  );
  const selectedProvider = providerOptions.find((provider) => provider.id === selectedProviderId) ?? null;
  const selectedModel = selectedProvider?.models.find((model) => model.id === input.model) ?? null;
  const configuredProviderModelCatalog = useMemo(
    () => providerModelCatalog.filter((catalog) => (
      providerOptions.some((provider) => provider.id === catalog.providerId && provider.configured)
    )),
    [providerModelCatalog, providerOptions]
  );
  const initialProvider = useMemo(() => {
    if (defaultProviderId === undefined) return null;
    return providerOptions.find((provider) => provider.id === defaultProviderId && provider.configured && provider.models.length > 0)
      ?? providerOptions.find((provider) => provider.configured && provider.models.length > 0)
      ?? null;
  }, [defaultProviderId, providerOptions]);

  const clearPendingPromptStream = (): void => {
    pendingPromptMarkdownRef.current = null;
    if (promptStreamFlushTimerRef.current !== null) {
      window.clearTimeout(promptStreamFlushTimerRef.current);
      promptStreamFlushTimerRef.current = null;
    }
  };

  const flushPendingPromptStream = (): void => {
    const promptMarkdown = pendingPromptMarkdownRef.current;
    pendingPromptMarkdownRef.current = null;
    if (promptStreamFlushTimerRef.current !== null) {
      window.clearTimeout(promptStreamFlushTimerRef.current);
      promptStreamFlushTimerRef.current = null;
    }
    if (promptMarkdown === null || !mountedRef.current) return;
    promptStreamAutoScrollRef.current = true;
    setInput((current) => {
      const next = { ...current, promptMarkdown };
      inputRef.current = next;
      return next;
    });
  };

  const setPromptMarkdown = (promptMarkdown: string): void => {
    setInput((current) => {
      const next = { ...current, promptMarkdown };
      inputRef.current = next;
      return next;
    });
  };

  const cancelPromptGeneration = (updateState = true): void => {
    const requestId = generationRequestIdRef.current;
    generationRequestIdRef.current = null;
    clearPendingPromptStream();
    if (updateState) setGeneratingPrompt(false);
    if (requestId && !generatePrompt) void window.beale.cancelResearchPromptGeneration(requestId).catch(() => undefined);
  };

  const generateFullPrompt = (inputOverride?: StartRunInput): Promise<string | null> => {
    const sessionInput = inputOverride ?? inputRef.current;
    const draft = sessionInput.promptMarkdown.trim();
    if (!draft || generatingPrompt) return Promise.resolve(null);
    cancelPromptGeneration();
    const requestId = clientRequestId('research_prompt');
    const workflowId = sessionInput.workflowId ?? defaultWorkflowId;
    const sourceStage = editorStage;
    generationRequestIdRef.current = requestId;
    generationSourceTextRef.current = draft;
    setGenerationError(null);
    setInput(() => {
      const next = {
        ...sessionInput,
        workflowId,
        goalObjective: sourceStage === 'goal' ? draft : sessionInput.goalObjective,
        promptMarkdown: ''
      };
      inputRef.current = next;
      return next;
    });
    setGeneratingPrompt(true);
    promptStreamAutoScrollRef.current = true;
    return (generatePrompt ?? window.beale.generateResearchPrompt)({
      requestId,
      operation: sourceStage === 'goal' ? 'expand_goal' : 'refine',
      researchPhase: workflowId,
      goalSentence: sourceStage === 'goal' ? draft : sessionInput.goalObjective,
      draftPromptMarkdown: sourceStage === 'prompt' ? draft : null,
      mode: sessionInput.mode,
      attemptStrategy: sessionInput.attemptStrategy,
      provider: researchProviderId(sessionInput.provider) ?? selectedProviderId ?? undefined,
      model: sessionInput.model,
      reasoningEffort: sessionInput.reasoningEffort,
      sandboxProfile: sessionInput.sandboxProfile,
      targetAssetId: sessionInput.targetAssetId ?? null,
      targetPath: sessionInput.targetPath ?? null
    })
      .then((generated) => {
        if (!mountedRef.current || generationRequestIdRef.current !== requestId) return null;
        clearPendingPromptStream();
        setPromptMarkdown(generated.promptMarkdown);
        setEditorStage('prompt');
        return generated.promptMarkdown;
      })
      .catch((caught: unknown) => {
        if (!mountedRef.current || generationRequestIdRef.current !== requestId) return null;
        clearPendingPromptStream();
        setPromptMarkdown(generationSourceTextRef.current ?? draft);
        const message = userFacingErrorMessage(caught);
        if (!/canceled/i.test(message)) setGenerationError(message);
        return null;
      })
      .finally(() => {
        if (!mountedRef.current || generationRequestIdRef.current !== requestId) return;
        generationRequestIdRef.current = null;
        generationSourceTextRef.current = null;
        setGeneratingPrompt(false);
      });
  };

  const selectWorkflow = (workflowId: ResearchGoalPhase): void => {
    if (!workflows.some((workflow) => workflow.id === workflowId)) return;
    update('workflowId', workflowId);
  };

  const selectGoalSentence = (sentence: string, phase: ResearchGoalPhase): void => {
    onSelectResearchGoalSuggestion(phase, sentence);
    cancelPromptGeneration();
    setEditorStage('goal');
    setGenerationError(null);
    setInput((current) => {
      const next = { ...current, workflowId: phase, goalObjective: sentence, promptMarkdown: sentence };
      inputRef.current = next;
      return next;
    });
  };

  useEffect(() => {
    cancelPromptGeneration();
    const next = researchSettingsInput(
      initialInput,
      initialWorkflowId,
      initialGoal,
      defaultShellSafetyMode
    );
    inputRef.current = next;
    setInput(next);
    const machineIds = next.targetIds?.length ? next.targetIds : next.machineId ? [next.machineId]
      : remoteTargetChoices?.find((target) => target.ready) ? [remoteTargetChoices.find((target) => target.ready)!.id] : [];
    selectedMachineIdsRef.current = machineIds;
    setSelectedMachineIds(machineIds);
    const inflatedProvider = researchProviderId(initialInput?.provider);
    setSelectedProviderId(inflatedProvider ?? defaultProviderId ?? null);
    providerSelectionInitializedRef.current = Boolean(inflatedProvider);
    modelSelectionInitializedRef.current = Boolean(initialInput?.model);
    setEditorStage(initialInput || initialGoal?.promptMarkdown ? 'prompt' : 'goal');
    setGenerationError(null);
  }, [defaultProviderId, defaultShellSafetyMode, formIdentity, initialGoal, initialInput, initialWorkflowId]);

  useEffect(() => {
    inputRef.current = input;
  }, [input]);

  useEffect(() => {
    if (!fleetWorkspaceId || remoteTargetChoices) return;
    let active = true;
    setFleetState(null);
    setFleetError(null);
    const updateFleet = (state: FleetState): void => {
      if (!active) return;
      setFleetState(state);
      setFleetError(null);
      const allowedTargets = allowedFleetTargets(state, fleetWorkspaceId);
      const valid = selectedMachineIdsRef.current.filter((machineId) => allowedTargets.some((target) => target.id === machineId));
      const machineIds = valid.length ? valid : [defaultFleetMachineId(state, fleetWorkspaceId)].filter((id): id is string => Boolean(id));
      selectedMachineIdsRef.current = machineIds;
      setSelectedMachineIds(machineIds);
      setInput((current) => {
        const next = { ...current, machineId: machineIds[0] };
        inputRef.current = next;
        return next;
      });
    };
    void loadWorkspaceFleetStateWithRetry(fleetWorkspaceId, (workspaceId) => window.beale.getFleetState(workspaceId))
      .then(updateFleet).catch((caught: unknown) => {
        if (active) setFleetError(userFacingErrorMessage(caught));
      });
    return () => { active = false; };
  }, [fleetWorkspaceId, formIdentity, initialInput?.machineId, Boolean(remoteTargetChoices), fleetLoadAttempt]);

  const remoteTargetKey = remoteTargetChoices?.map((target) => `${target.id}:${target.ready}`).join('|');
  useEffect(() => {
    if (!remoteTargetChoices) return;
    const valid = selectedMachineIdsRef.current.filter((id) => remoteTargetChoices.some((target) => target.id === id));
    const fallback = remoteTargetChoices.find((target) => target.ready)?.id ?? remoteTargetChoices[0]?.id;
    const machineIds = valid.length ? valid : fallback ? [fallback] : [];
    selectedMachineIdsRef.current = machineIds;
    setSelectedMachineIds(machineIds);
    setFleetError(null);
  }, [formIdentity, remoteTargetKey]);

  useEffect(() => {
    mountedRef.current = true;
    const unsubscribe = window.beale.onResearchPromptGenerationUpdate((update) => {
      if (!mountedRef.current || generationRequestIdRef.current !== update.requestId) return;
      pendingPromptMarkdownRef.current = update.promptMarkdown;
      if (promptStreamFlushTimerRef.current !== null) return;
      promptStreamFlushTimerRef.current = window.setTimeout(flushPendingPromptStream, PROMPT_STREAM_RENDER_INTERVAL_MS);
    });
    return () => {
      mountedRef.current = false;
      unsubscribe();
      cancelPromptGeneration(false);
    };
  }, []);

  useEffect(() => {
    if (providerSelectionInitializedRef.current || !initialProvider) return;
    providerSelectionInitializedRef.current = true;
    setSelectedProviderId(initialProvider.id);
  }, [initialProvider]);

  useEffect(() => {
    if (!selectedProvider || defaultProviderId === undefined || providerModelDefaults === undefined) return;
    setInput((current) => {
      const preferredModelId = providerDefaultModel(selectedProvider.id, openAiStatus, researchProviderStatuses, providerModelDefaults);
      const model = (!modelSelectionInitializedRef.current
        ? selectedProvider.models.find((candidate) => candidate.id === preferredModelId)
        : selectedProvider.models.find((candidate) => candidate.id === current.model))
        ?? selectedProvider.models.find((candidate) => candidate.id === preferredModelId)
        ?? selectedProvider.models[0];
      if (!model) return current;
      const defaultEffort = providerModelDefaults[selectedProvider.id]?.reasoningEffort
        ?? effortLevelFromInput(current.reasoningEffort);
      const effort = inputValueForEffort(preferredEffort(model.effortLevels, defaultEffort));
      modelSelectionInitializedRef.current = true;
      if (current.provider === selectedProvider.id && current.model === model.id && current.reasoningEffort === effort) {
        return current;
      }
      const { daybreakBlue: _previousDaybreakBlue, ...otherInput } = current;
      return {
        ...otherInput,
        provider: selectedProvider.id,
        model: model.id,
        reasoningEffort: effort,
        fastMode: selectedProvider.id === 'openai-codex' && current.fastMode === true,
        ...(selectedProvider.id === 'openai-codex' ? { daybreakBlue: true } : {})
      };
    });
  }, [defaultProviderId, openAiStatus, providerModelDefaults, researchProviderStatuses, selectedProvider]);

  useEffect(() => {
    if (providerModelDefaults === undefined || providerPolicyRiskAcknowledgements === undefined || providerOptions.length === 0) return;
    setInput((current) => {
      const currentCollaboration = normalizeResearchCollaboration(current.collaboration);
      const existing = new Map(currentCollaboration.providers.map((preference) => [
        collaboratorKey(preference.provider, preference.model),
        preference
      ]));
      const providers = providerOptions.flatMap((provider) => {
        const preferredModelId = providerDefaultModel(provider.id, openAiStatus, researchProviderStatuses, providerModelDefaults);
        const defaultEffort = providerModelDefaults[provider.id]?.reasoningEffort ?? 'high';
        const ready = provider.configured && providerPolicyReady(provider.id, providerPolicyRiskAcknowledgements);
        const orderedModels = [...provider.models].sort((left, right) => {
          if (left.id === preferredModelId) return -1;
          if (right.id === preferredModelId) return 1;
          return 0;
        });
        return orderedModels.map((model) => {
          const stored = existing.get(collaboratorKey(provider.id, model.id));
          return {
            provider: provider.id,
            model: model.id,
            reasoningEffort: stored?.reasoningEffort ?? preferredEffort(model.effortLevels, defaultEffort),
            enabled: ready && stored?.enabled === true,
            roles: stored?.roles
          };
        });
      });
      const leadModelId = current.model ?? '';
      const leadProvider = providerOptions.find((provider) => provider.id === current.provider)
        ?? providerOptions.find((provider) => provider.id === selectedProviderId);
      const leadProviderId = leadProvider?.id ?? selectedProviderId;
      const leadModel = leadProvider?.models.find((model) => model.id === leadModelId);
      const leadEffort = effortLevelFromInput(current.reasoningEffort);
      const leadReady = leadProviderId !== null
        && leadProvider?.configured === true
        && providerPolicyReady(leadProviderId, providerPolicyRiskAcknowledgements)
        && leadModel?.effortLevels.includes(leadEffort) === true;
      const candidateCollaboration = { ...currentCollaboration, providers };
      const collaboration = leadReady
        ? ensureDefaultResearchCollaborator(candidateCollaboration, {
          provider: leadProviderId,
          model: leadModelId,
          reasoningEffort: leadEffort,
          enabled: true
        })
        : candidateCollaboration;
      const next = { ...current, collaboration };
      inputRef.current = next;
      return next;
    });
  }, [openAiStatus, providerModelDefaults, providerOptions, providerPolicyRiskAcknowledgements, researchProviderStatuses, selectedProviderId]);

  useLayoutEffect(() => {
    if (!generatingPrompt || !promptStreamAutoScrollRef.current) return;
    const promptBox = promptBoxRef.current;
    if (promptBox) promptBox.scrollTop = promptBox.scrollHeight;
  }, [generatingPrompt, input.promptMarkdown]);

  const update = <K extends keyof StartRunInput>(key: K, value: StartRunInput[K]): void => {
    setInput((current) => {
      const next: StartRunInput = { ...current, [key]: value };
      inputRef.current = next;
      return next;
    });
    if (key === 'promptMarkdown') {
      promptStreamAutoScrollRef.current = false;
      setGenerationError(null);
    }
  };

  const selectMachine = (machineId: string): void => {
    const nextIds = toggleResearchTargetSelection(selectedMachineIdsRef.current, machineId);
    selectedMachineIdsRef.current = nextIds;
    setSelectedMachineIds(nextIds);
    setInput((current) => {
      const next = { ...current, machineId: nextIds[0] };
      inputRef.current = next;
      return next;
    });
  };

  const hasPromptDraft = input.promptMarkdown.trim().length > 0;
  const activeWorkflowId = input.workflowId ?? defaultWorkflowId;
  const selectedEffort = effortLevelFromInput(input.reasoningEffort);
  const repeatSchedule = normalizeRepeatSchedule(input.budget.repeatSchedule);
  const requiresCyberPolicyAcknowledgement = requireCyberPolicyAcknowledgement || collaborationRequiresCyberPolicyAcknowledgement(profile?.id);
  const collaboration = normalizeResearchCollaboration(input.collaboration);
  const enabledCollaborators = collaboration.providers.filter((provider) => provider.enabled);
  const availableCollaborators = collaboration.providers.filter((candidate) => {
    if (candidate.enabled) return false;
    const provider = providerOptions.find((option) => option.id === candidate.provider);
    return provider?.configured === true
      && provider.models.length > 0
      && (!requiresCyberPolicyAcknowledgement || providerPolicyReady(candidate.provider, providerPolicyRiskAcknowledgements));
  });
  const nextCollaborator = selectNextAvailableCollaborator(
    availableCollaborators,
    enabledCollaborators,
    selectedProviderId
  );
  const collaborationReady = enabledCollaborators.every((preference) => {
      const provider = providerOptions.find((candidate) => candidate.id === preference.provider);
      return provider?.configured === true
        && provider.models.some((model) => model.id === preference.model && model.effortLevels.includes(preference.reasoningEffort))
        && (!requiresCyberPolicyAcknowledgement || providerPolicyReady(preference.provider, providerPolicyRiskAcknowledgements));
    });
  const canGenerate = hasPromptDraft && selectedProvider?.configured === true && !generatingPrompt;
  const vmRepeatConflict = selectedMachineIds.some((machineId) => machineId.startsWith('vm:')) && repeatSchedule.type !== 'none';
  const remoteTargetsReady = !remoteTargetChoices || selectedMachineIds.length > 0
    && selectedMachineIds.every((machineId) => remoteTargetChoices.some((target) => target.id === machineId && target.ready));
  const canStart = (trackMode || hasPromptDraft)
    && (!fleetWorkspaceId || Boolean(selectedMachineIds.length > 0 && selectedMachineIds.every((machineId) => (
      remoteTargetChoices
        ? remoteTargetChoices.some((target) => target.id === machineId && target.ready)
        : fleetState && isAllowedFleetMachine(fleetState, fleetWorkspaceId, machineId)
          && allowedFleetTargets(fleetState, fleetWorkspaceId).find((target) => target.id === machineId)?.ready
    ))))
    && !vmRepeatConflict
    && selectedProvider?.configured === true
    && Boolean(selectedModel?.effortLevels.includes(selectedEffort))
    && collaborationReady;

  const startWithInput = (startInput: StartRunInput): void => {
    if (startingRun) return;
    setStartingRun(true);
    const targetIds = [...selectedMachineIdsRef.current];
    void Promise.resolve(onSubmit({ ...startInput,
      ...(fleetWorkspaceId ? { machineId: targetIds.length > 1 ? 'local' : targetIds[0], targetIds } : {}),
      goalObjective: startInput.goalEnabled ? resolveGoalObjective(startInput.goalObjective, startInput.promptMarkdown) : null
    })).finally(() => setStartingRun(false));
  };

  const start = (): void => {
    const current = inputRef.current;
    startWithInput({
      ...current,
      goalObjective: current.goalEnabled
        ? resolveGoalObjective(current.goalObjective, current.promptMarkdown)
        : null
    });
  };

  const generateAndStart = (): void => {
    void generateFullPrompt().then((promptMarkdown) => {
      if (!promptMarkdown) return;
      const current = inputRef.current;
      startWithInput({
        ...current,
        promptMarkdown,
        goalObjective: current.goalEnabled
          ? resolveGoalObjective(current.goalObjective, promptMarkdown)
          : null
      });
    });
  };

  const selectProvider = (providerId: ResearchModelProviderId): void => {
    providerSelectionInitializedRef.current = true;
    modelSelectionInitializedRef.current = true;
    setSelectedProviderId(providerId);
    const provider = providerOptions.find((candidate) => candidate.id === providerId);
    const preferredModelId = providerDefaultModel(providerId, openAiStatus, researchProviderStatuses, providerModelDefaults ?? {});
    const model = provider?.models.find((candidate) => candidate.id === preferredModelId) ?? provider?.models[0];
    if (!model) return;
    setInput((current) => {
      const { daybreakBlue: _previousDaybreakBlue, ...otherInput } = current;
      const next = {
        ...otherInput,
        provider: providerId,
        model: model.id,
        fastMode: providerId === 'openai-codex' && current.fastMode === true,
        ...(providerId === 'openai-codex' ? { daybreakBlue: true } : {}),
        reasoningEffort: inputValueForEffort(preferredEffort(
          model.effortLevels,
          providerModelDefaults?.[providerId]?.reasoningEffort ?? effortLevelFromInput(current.reasoningEffort)
        ))
      };
      inputRef.current = next;
      return next;
    });
  };

  const selectModel = (modelId: string): void => {
    const model = selectedProvider?.models.find((candidate) => candidate.id === modelId);
    if (!model) return;
    setInput((current) => {
      const { daybreakBlue: _previousDaybreakBlue, ...otherInput } = current;
      const next = {
        ...otherInput,
        model: model.id,
        ...(selectedProviderId === 'openai-codex' ? { daybreakBlue: true } : {}),
        reasoningEffort: inputValueForEffort(preferredEffort(model.effortLevels, effortLevelFromInput(current.reasoningEffort)))
      };
      inputRef.current = next;
      return next;
    });
  };

  const selectEffort = (effort: ResearchModelEffortLevel): void => {
    update('reasoningEffort', inputValueForEffort(effort));
  };

  const selectRepeatSchedule = (repeatSchedule: RepeatSchedule): void => {
    if (disableNoRepeat && normalizeRepeatSchedule(repeatSchedule).type === 'none') return;
    setInput((current) => {
      const next = {
        ...current,
        budget: {
          ...current.budget,
          repeatSchedule: normalizeRepeatSchedule(repeatSchedule)
        }
      };
      inputRef.current = next;
      return next;
    });
  };

  const selectSubagentMode = (subagentMode: ResearchSubagentMode): void => {
    update('collaboration', { ...collaboration, subagentMode });
  };

  const updateCollaborator = (
    providerId: ResearchModelProviderId,
    modelId: string,
    patch: Partial<(typeof collaboration.providers)[number]>
  ): void => {
    update('collaboration', {
      ...collaboration,
      providers: collaboration.providers.map((preference) => (
        preference.provider === providerId && preference.model === modelId
        ? { ...preference, ...patch }
        : preference
      ))
    });
  };

  const addCollaborator = (): void => {
    if (!nextCollaborator) return;
    update('collaboration', {
      ...collaboration,
      providers: enableCollaboratorAtTop(collaboration.providers, nextCollaborator)
    });
  };

  const removeCollaborator = (providerId: ResearchModelProviderId, modelId: string): void => {
    updateCollaborator(providerId, modelId, { enabled: false });
  };

  const selectCollaboratorProvider = (
    currentProviderId: ResearchModelProviderId,
    currentModelId: string,
    nextProviderId: ResearchModelProviderId
  ): void => {
    if (currentProviderId === nextProviderId) return;
    const preferredModelId = providerDefaultModel(
      nextProviderId,
      openAiStatus,
      researchProviderStatuses,
      providerModelDefaults ?? {}
    );
    const target = collaboration.providers.find((preference) => (
      preference.provider === nextProviderId
      && preference.model === preferredModelId
      && !preference.enabled
    )) ?? collaboration.providers.find((preference) => (
      preference.provider === nextProviderId && !preference.enabled
    ));
    if (!target) return;
    update('collaboration', {
      ...collaboration,
      providers: collaboration.providers.map((preference) => {
        if (preference.provider === currentProviderId && preference.model === currentModelId) {
          return { ...preference, enabled: false };
        }
        if (preference.provider === target.provider && preference.model === target.model) {
          return { ...preference, enabled: true };
        }
        return preference;
      })
    });
  };

  const selectCollaboratorModel = (
    providerId: ResearchModelProviderId,
    currentModelId: string,
    nextModelId: string
  ): void => {
    if (currentModelId === nextModelId) return;
    const target = collaboration.providers.find((preference) => (
      preference.provider === providerId && preference.model === nextModelId && !preference.enabled
    ));
    if (!target) return;
    update('collaboration', {
      ...collaboration,
      providers: collaboration.providers.map((preference) => {
        if (preference.provider === providerId && preference.model === currentModelId) {
          return { ...preference, enabled: false };
        }
        if (preference.provider === providerId && preference.model === nextModelId) {
          return { ...preference, enabled: true };
        }
        return preference;
      })
    });
  };

  const closeForm = (): void => {
    cancelPromptGeneration();
    onCancel();
  };

  if (formPresentation === 'session') {
    const generationFeedback = generatingPrompt
      ? 'Adding useful context…'
      : generationError
        ? `Could not add context: ${generationError}`
        : null;
    const initialModelSelection: ResearchModelSelection | undefined = selectedProviderId && selectedModel ? {
      provider: selectedProviderId,
      model: selectedModel.id,
      reasoningEffort: selectedEffort,
      fastMode: selectedProviderId === 'openai-codex' && input.fastMode === true,
      ...(selectedProviderId === 'openai-codex' ? { daybreakBlue: true } : {})
    } : undefined;
    const startFromSessionComposer = (
      promptMarkdown: string,
      modelSelection: ResearchModelSelection,
      shellSafetyMode: ShellSafetyMode
    ): void => {
      const { daybreakBlue: _previousDaybreakBlue, ...otherInput } = inputRef.current;
      const next: StartRunInput = {
        ...otherInput,
        promptMarkdown: trackMode ? '' : promptMarkdown,
        provider: modelSelection.provider,
        model: modelSelection.model,
        reasoningEffort: inputValueForEffort(modelSelection.reasoningEffort),
        fastMode: modelSelection.provider === 'openai-codex' && modelSelection.fastMode === true,
        ...(modelSelection.provider === 'openai-codex' ? { daybreakBlue: true } : {}),
        shellSafetyMode
      };
      inputRef.current = next;
      setInput(next);
      setSelectedProviderId(modelSelection.provider);
      if (generateEnabled) generateAndStart();
      else start();
    };

    return (
      <CommentaryView
        state="new-research"
        selectedRunId={null}
        detail={null}
        events={[]}
        activeScope={null}
        providerModelCatalog={configuredProviderModelCatalog}
        providerModelDefaults={providerModelDefaults}
        busy={busy || startingRun || generatingPrompt || !remoteTargetsReady || vmRepeatConflict || providerOptions.length === 0}
        showBackToMain={false}
        searchHighlightQuery=""
        initialModelSelection={initialModelSelection}
        collaboration={collaboration}
        onCollaborationChange={(nextCollaboration) => update('collaboration', nextCollaboration)}
        initialSafetyMode={input.shellSafetyMode}
        initialInstruction={trackMode ? '' : input.promptMarkdown}
        inputPlaceholder={trackMode ? 'Continue security research until all surfaces are covered.' : newResearchPromptPlaceholder(generateEnabled)}
        instructionLocked={trackMode}
        ariaLabel={title ?? 'Start new research'}
        emptyContent={(
          <NewResearchWelcome
            key={formIdentity}
            workspaceName={workspaceName}
            workflows={workflows}
            suggestions={researchGoalSuggestions}
            loading={researchGoalSuggestionsLoading}
            errors={researchGoalSuggestionErrors}
            visible={showSuggestions}
            onOpenWorkflow={(workflowId) => {
              selectWorkflow(workflowId);
              onLoadResearchGoalSuggestions(workflowId);
            }}
            onSelect={selectGoalSentence}
            onRegenerate={onRetryResearchGoalSuggestions}
          />
        )}
        preComposerContent={(
          <>
            <div className="new-research-options-tray" aria-label="New research options">
              <div className="new-research-options-tray-left">
                <span className="new-research-app-server-name" title={appServerName}>
                  <Monitor size={14} aria-hidden="true" />
                  <span>{appServerName}</span>
                </span>
                <ArrowRight className="new-research-target-arrow" size={13} aria-hidden="true" />
                {fleetWorkspaceId ? <MachinePicker targets={remoteTargetChoices ?? (fleetState ? allowedFleetTargets(fleetState, fleetWorkspaceId) : null)} values={selectedMachineIds} disabled={generatingPrompt} error={fleetError} onChange={selectMachine} onRetry={() => setFleetLoadAttempt((attempt) => attempt + 1)} /> : <span>No target</span>}
              </div>
              {!trackMode ? <div className="new-research-options-tray-right">
                <NewResearchSettingsMenu
                  goalEnabled={input.goalEnabled}
                  addContextEnabled={generateEnabled}
                  showAddContext={showAddContext}
                  repeatSchedule={repeatSchedule}
                  disabled={generatingPrompt}
                  disableNoRepeat={disableNoRepeat}
                  autoOpenSchedule={autoOpenSchedule}
                  onGoalChange={(enabled) => update('goalEnabled', enabled)}
                  onAddContextChange={setGenerateEnabled}
                  onRepeatChange={selectRepeatSchedule}
                />
              </div> : null}
            </div>
            {vmRepeatConflict ? <div role="alert">Repeating sessions are available on Local only. Choose No repeat to use this VM.</div> : null}
          </>
        )}
        postComposerContent={generationFeedback ? (
          <div
            className="new-research-session-feedback"
            role={generationError ? 'alert' : 'status'}
          >
            {generationFeedback}
          </div>
        ) : null}
        onBackToMain={() => undefined}
        onInitialInstruction={startFromSessionComposer}
        onCancel={closeForm}
        onSessionAction={() => undefined}
        onSteerInstruction={() => undefined}
      />
    );
  }

  const primaryLabel = formPresentation === 'dialog' ? 'Start' : submitLabel;
  const actions = (
    <>
      {generateEnabled ? (
        <button type="button" disabled={busy || startingRun || !canGenerate} onClick={() => void generateFullPrompt()}>
          <Sparkles size={15} />
          Add Context
        </button>
      ) : null}
      <button
        className="primary-button"
        type="button"
        disabled={busy || startingRun || generatingPrompt || !canStart}
        onClick={generateEnabled ? generateAndStart : start}
      >
        {generateEnabled ? <Sparkles size={16} /> : formPresentation === 'dialog' ? <Play size={16} /> : null}
        {generateEnabled ? `Add Context & ${primaryLabel}` : primaryLabel}
      </button>
    </>
  );
  const content = (
      <div className={`start-run-modal-body research-settings-form-body ${showSuggestions ? '' : 'without-suggestions'}`.trim()}>
        <div className={`new-research-compose-layout ${showSuggestions ? '' : 'without-suggestions'}`.trim()}>
          <section className="new-research-composer" aria-label="Research objective composer" aria-busy={generatingPrompt}>
            <textarea
              ref={promptBoxRef}
              autoFocus={formPresentation === 'dialog'}
              value={input.promptMarkdown}
              disabled={generatingPrompt}
              placeholder={editorStage === 'goal'
                ? 'Describe the research outcome you want.'
                : 'Review and edit the enriched objective.'}
              aria-label={editorStage === 'goal' ? 'Research goal' : 'Research objective brief'}
              onChange={(event) => update('promptMarkdown', event.target.value)}
            />
            <div className="new-research-composer-feedback" aria-live="polite">
              {generatingPrompt ? 'Adding useful context…' : generationError ? `Could not add context: ${generationError}` : ''}
            </div>
            <div className="new-research-composer-actions">
              <RepeatSchedulePicker
                value={repeatSchedule}
                disabled={generatingPrompt}
                disableNoRepeat={disableNoRepeat}
                autoOpen={autoOpenSchedule}
                onChange={selectRepeatSchedule}
              />
              <label
                className="new-research-goal-toggle"
                title="Keep working across turns until the objective is complete or genuinely blocked."
              >
                <input
                  type="checkbox"
                  checked={input.goalEnabled}
                  disabled={generatingPrompt}
                  onChange={(event) => update('goalEnabled', event.target.checked)}
                />
                <span>Goal</span>
              </label>
              {showAddContext ? (
                <label
                  className="new-research-generate-toggle"
                  title="Expand this into a campaign-aware, ambitious objective while leaving the research method open."
                >
                  <input
                    type="checkbox"
                    checked={generateEnabled}
                    disabled={generatingPrompt}
                    onChange={(event) => setGenerateEnabled(event.target.checked)}
                  />
                  <span>Add Context</span>
                </label>
              ) : null}
              {fleetWorkspaceId ? <MachinePicker targets={remoteTargetChoices ?? (fleetState ? allowedFleetTargets(fleetState, fleetWorkspaceId) : null)} values={selectedMachineIds} disabled={generatingPrompt} error={fleetError} onChange={selectMachine} onRetry={() => setFleetLoadAttempt((attempt) => attempt + 1)} /> : null}
            </div>
            {vmRepeatConflict ? <div role="alert">Repeating sessions are available on Local only. Choose No repeat to use this VM.</div> : null}
          </section>
          {showSuggestions ? (
            <ResearchGoalChooser
              workflows={workflows}
              selectedWorkflowId={activeWorkflowId}
              onSelectWorkflow={selectWorkflow}
              suggestions={researchGoalSuggestions}
              loading={researchGoalSuggestionsLoading}
              errors={researchGoalSuggestionErrors}
              onLoad={onLoadResearchGoalSuggestions}
              onSelect={selectGoalSentence}
              onRetry={onRetryResearchGoalSuggestions}
            />
          ) : null}
        </div>
        <div className="collaboration-settings">
            <div className="research-model-team">
              <div className="research-model-team-column research-lead-model-column">
                <span className="research-model-team-label">Lead</span>
                <ModelSelectionPicker
                  className="research-model-squircle research-lead-model-picker"
                  providerValue={selectedProviderId ?? ''}
                  modelValue={selectedModel?.id ?? ''}
                  effortValue={selectedEffort}
                  fastModeValue={selectedProviderId === 'openai-codex' ? input.fastMode === true : undefined}
                  title="Lead provider, model, and effort"
                  ariaLabel="Lead model settings"
                  disabled={!selectedModel || generatingPrompt}
                  providerOptions={providerOptions.map((provider) => ({
                    value: provider.id,
                    label: provider.label,
                    disabled: provider.models.length === 0
                  }))}
                  modelOptions={(selectedProvider?.models ?? []).map((model) => ({
                    value: model.id,
                    label: selectedProviderId ? researchModelNameLabel(selectedProviderId, model.name) : model.name
                  }))}
                  effortOptions={(selectedModel?.effortLevels ?? []).map((effort) => ({ value: effort, label: effortLabel(effort) }))}
                  onSelectProvider={(value) => selectProvider(value as ResearchModelProviderId)}
                  onSelectModel={selectModel}
                  onSelectEffort={(value) => selectEffort(value as ResearchModelEffortLevel)}
                  onSelectFastMode={(enabled) => update('fastMode', enabled)}
                />
              </div>
              <div className="research-model-team-column research-collaborator-model-column">
                <span className="research-model-team-label">Collaborators</span>
                <div className="research-collaborator-stack">
                  {enabledCollaborators.map((preference) => {
                    const provider = providerOptions.find((candidate) => candidate.id === preference.provider);
                    const model = provider?.models.find((candidate) => candidate.id === preference.model) ?? null;
                    const canRemove = enabledCollaborators.length > 1;
                    return (
                      <div
                        className="research-model-squircle research-collaborator-squircle"
                        key={collaboratorKey(preference.provider, preference.model)}
                      >
                        <ModelSelectionPicker
                          className="research-collaborator-picker"
                          providerValue={preference.provider}
                          modelValue={preference.model}
                          effortValue={preference.reasoningEffort}
                          title={`${provider?.label ?? preference.provider} collaborator settings`}
                          ariaLabel={`${provider?.label ?? preference.provider} collaborator model settings`}
                          disabled={generatingPrompt}
                          providerOptions={providerOptions.map((candidate) => ({
                            value: candidate.id,
                            label: candidate.label,
                            disabled: candidate.models.length === 0
                              || !candidate.configured
                              || !providerPolicyReady(candidate.id, providerPolicyRiskAcknowledgements)
                              || !collaboration.providers.some((available) => (
                                available.provider === candidate.id
                                && (candidate.id === preference.provider || !available.enabled)
                              ))
                          }))}
                          modelOptions={(provider?.models ?? []).map((candidate) => ({
                            value: candidate.id,
                            label: researchModelNameLabel(preference.provider, candidate.name),
                            disabled: candidate.id !== preference.model
                              && enabledCollaborators.some((enabled) => (
                                enabled.provider === preference.provider && enabled.model === candidate.id
                              ))
                          }))}
                          effortOptions={(model?.effortLevels ?? []).map((effort) => ({ value: effort, label: effortLabel(effort) }))}
                          onSelectProvider={(value) => selectCollaboratorProvider(
                            preference.provider,
                            preference.model,
                            value as ResearchModelProviderId
                          )}
                          onSelectModel={(modelId) => selectCollaboratorModel(
                            preference.provider,
                            preference.model,
                            modelId
                          )}
                          onSelectEffort={(effort) => updateCollaborator(preference.provider, preference.model, {
                            reasoningEffort: effort as ResearchModelEffortLevel
                          })}
                        />
                        <button
                          type="button"
                          className="research-collaborator-remove"
                          title={canRemove
                            ? `Remove ${provider?.label ?? preference.provider} collaborator`
                            : 'Add another collaborator before removing this one'}
                          aria-label={`Remove ${provider?.label ?? preference.provider} collaborator`}
                          disabled={generatingPrompt || !canRemove}
                          onClick={() => removeCollaborator(preference.provider, preference.model)}
                        >
                          <X size={14} />
                        </button>
                      </div>
                    );
                  })}
                  <button
                    type="button"
                    className="research-model-squircle research-collaborator-add"
                    title={nextCollaborator ? 'Add collaborator' : 'No additional acknowledged providers are available'}
                    aria-label="Add collaborator"
                    disabled={!nextCollaborator || generatingPrompt}
                    onClick={addCollaborator}
                  >
                    <Plus size={18} />
                  </button>
                </div>
              </div>
            </div>
            <div className="collaboration-controls-row">
              <div className="collaboration-controls-right">
                <label className="collaboration-inline-control">
                  <span title="Simple provides direct subagents for bounded delegation. Advanced coordinates sustained parallel discovery with role-based Discoverers, Provers, Reviewers, and Reporters.">Subagents</span>
                  <select
                    value={collaboration.subagentMode}
                    onChange={(event) => selectSubagentMode(event.target.value as ResearchSubagentMode)}
                  >
                    <option value="simple">Simple</option>
                    <option value="advanced">Advanced</option>
                  </select>
                </label>
              </div>
            </div>
            {!collaborationReady ? (
              <div className="policy-line collaboration-readiness-warning" role="alert">
                <ShieldAlert size={14} /> Every selected collaborator must be authenticated and use a supported model and effort.{requiresCyberPolicyAcknowledgement ? ' Cybersecurity collaborators must also have their policy acknowledgement accepted in Provider settings.' : ''}
              </div>
            ) : null}
        </div>
      </div>
  );

  return formPresentation === 'dialog' ? (
    <Modal
      title={title ?? profilePresentation?.newResearchLabel ?? 'New Research'}
      wide
      className="start-run-dialog"
      onClose={closeForm}
      footer={actions}
    >
      {content}
    </Modal>
  ) : (
    <div className="research-settings-form" aria-label={title ?? 'Research settings'}>
      {content}
      <div className="research-settings-form-actions">{actions}</div>
    </div>
  );
}

type NewResearchSettingsSection = 'goal' | 'context' | 'repeat';

function NewResearchSettingsMenu({
  goalEnabled,
  addContextEnabled,
  showAddContext,
  repeatSchedule,
  disabled,
  disableNoRepeat,
  autoOpenSchedule,
  onGoalChange,
  onAddContextChange,
  onRepeatChange
}: {
  goalEnabled: boolean;
  addContextEnabled: boolean;
  showAddContext: boolean;
  repeatSchedule: RepeatSchedule;
  disabled: boolean;
  disableNoRepeat: boolean;
  autoOpenSchedule: boolean;
  onGoalChange: (enabled: boolean) => void;
  onAddContextChange: (enabled: boolean) => void;
  onRepeatChange: (schedule: RepeatSchedule) => void;
}): JSX.Element {
  const [open, setOpen] = useState(autoOpenSchedule);
  const [section, setSection] = useState<'repeat' | null>(autoOpenSchedule ? 'repeat' : null);
  const pickerRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const submenuRef = useRef<HTMLDivElement | null>(null);
  const normalizedSchedule = normalizeRepeatSchedule(repeatSchedule);
  const interval = normalizedSchedule.type === 'none' ? 1 : normalizedSchedule.interval;
  const sections: { id: NewResearchSettingsSection; label: string; value?: string; checked?: boolean }[] = [
    { id: 'goal', label: 'Goal Mode', checked: goalEnabled },
    ...(showAddContext ? [{ id: 'context' as const, label: 'Add Context', checked: addContextEnabled }] : []),
    { id: 'repeat', label: 'Repeat', value: repeatScheduleLabel(normalizedSchedule) }
  ];
  const close = (restoreFocus = false): void => {
    setOpen(false);
    setSection(null);
    if (restoreFocus) window.requestAnimationFrame(() => triggerRef.current?.focus());
  };

  useEffect(() => {
    if (!open) return undefined;
    const dismissOutside = (event: PointerEvent): void => {
      if (!pickerRef.current?.contains(event.target as Node)) close();
    };
    const dismissEscape = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') { event.preventDefault(); close(true); }
    };
    document.addEventListener('pointerdown', dismissOutside);
    document.addEventListener('keydown', dismissEscape);
    return () => {
      document.removeEventListener('pointerdown', dismissOutside);
      document.removeEventListener('keydown', dismissEscape);
    };
  }, [open]);

  useEffect(() => { if (disabled) close(); }, [disabled]);
  useEffect(() => {
    if (!open || !autoOpenSchedule) return;
    window.requestAnimationFrame(() => menuRef.current?.focus());
  }, [open, autoOpenSchedule]);

  const focusSubmenu = (): void => {
    window.requestAnimationFrame(() => submenuRef.current?.querySelector<HTMLElement>('button:not(:disabled), input:not(:disabled)')?.focus());
  };
  const moveSubmenuFocus = (event: ReactKeyboardEvent<HTMLButtonElement>): void => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault();
    const options = Array.from(submenuRef.current?.querySelectorAll<HTMLButtonElement>('button.new-research-settings-option:not(:disabled)') ?? []);
    const index = options.indexOf(event.currentTarget);
    options[(index + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length]?.focus();
  };

  return <div className={`new-research-settings${open ? ' is-open' : ''}`} ref={pickerRef}>
    <button type="button" ref={triggerRef} className="new-research-settings-trigger" aria-label="New research settings"
      title="New research settings" aria-haspopup="menu" aria-expanded={open} disabled={disabled}
      onClick={() => { setOpen((current) => !current); setSection(null); }}
      onKeyDown={(event) => {
        if (event.key === 'ArrowDown') {
          event.preventDefault();
          setOpen(true);
          setSection(null);
          window.requestAnimationFrame(() => menuRef.current?.querySelector<HTMLButtonElement>('.new-research-settings-row')?.focus());
        }
      }}>
      <Settings size={16} aria-hidden="true" />
    </button>
    {open ? <div className="new-research-settings-menu" role="menu" aria-label="New research settings" tabIndex={-1} ref={menuRef}>
      {sections.map((item, index) => <button type="button" role={item.id === 'repeat' ? 'menuitem' : 'menuitemcheckbox'}
        aria-checked={item.id === 'repeat' ? undefined : item.checked}
        aria-haspopup={item.id === 'repeat' ? 'menu' : undefined}
        aria-expanded={item.id === 'repeat' ? section === 'repeat' : undefined}
        className={`new-research-settings-row${section === item.id ? ' is-active' : ''}`} key={item.id}
        onMouseEnter={() => setSection(item.id === 'repeat' ? 'repeat' : null)}
        onFocus={() => setSection(item.id === 'repeat' ? 'repeat' : null)}
        onClick={() => {
          if (item.id === 'goal') { onGoalChange(!goalEnabled); setSection(null); }
          else if (item.id === 'context') { onAddContextChange(!addContextEnabled); setSection(null); }
          else { setSection('repeat'); focusSubmenu(); }
        }}
        onKeyDown={(event) => {
          if (event.key === 'ArrowRight') {
            event.preventDefault();
            if (item.id === 'repeat') { setSection('repeat'); focusSubmenu(); }
          }
          if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
            event.preventDefault();
            const next = sections[(index + (event.key === 'ArrowDown' ? 1 : -1) + sections.length) % sections.length];
            menuRef.current?.querySelectorAll<HTMLButtonElement>('.new-research-settings-row')[sections.indexOf(next!)]?.focus();
          }
        }}>
        <span>{item.label}</span>
        {item.id === 'repeat' ? <><span className="new-research-settings-value">{item.value}</span><ChevronRight size={13} aria-hidden="true" /></>
          : <span className={`menu-row-checkbox${item.checked ? ' is-checked' : ''}`} aria-hidden="true">{item.checked ? <Check size={11} /> : null}</span>}
      </button>)}
      {section === 'repeat' ? <div className="new-research-settings-submenu" role="menu" aria-label="Repeat" ref={submenuRef}
        onKeyDown={(event) => {
          if (event.key !== 'ArrowLeft') return;
          event.preventDefault();
          menuRef.current?.querySelectorAll<HTMLButtonElement>('.new-research-settings-row')[sections.findIndex((item) => item.id === section)]?.focus();
        }}>
        {REPEAT_SCHEDULE_TYPES.map((type) => <button type="button" role="menuitemradio" aria-checked={normalizedSchedule.type === type}
          className="new-research-settings-option" disabled={disableNoRepeat && type === 'none'} key={type}
          onKeyDown={moveSubmenuFocus}
          onClick={() => onRepeatChange(repeatScheduleFor(type, type === normalizedSchedule.type ? interval : 1))}>
          <span>{repeatTypeLabel(type)}</span>{normalizedSchedule.type === type ? <Check size={13} aria-hidden="true" /> : null}
        </button>)}
        <div className="new-research-settings-repeat-interval">
          <label htmlFor="new-research-repeat-interval">Every</label>
          <input id="new-research-repeat-interval" type="number" min={1} max={99} step={1} value={interval}
            disabled={normalizedSchedule.type === 'none'}
            onChange={(event) => onRepeatChange(repeatScheduleFor(normalizedSchedule.type === 'none' ? 'daily' : normalizedSchedule.type, Number(event.target.value)))} />
          <select value={repeatScheduleUnit(normalizedSchedule.type)} aria-label="Repeat unit" disabled={normalizedSchedule.type === 'none'}
            onChange={(event) => onRepeatChange(repeatScheduleFor(repeatScheduleTypeForUnit(event.target.value as RepeatScheduleUnit), interval))}>
            {(['minute', 'hour', 'day', 'week', 'month'] as const).map((unit) => <option value={unit} key={unit}>{unit}{interval === 1 ? '' : 's'}</option>)}
          </select>
        </div>
      </div> : null}
    </div> : null}
  </div>;
}

function RepeatSchedulePicker({
  value,
  disabled,
  disableNoRepeat,
  autoOpen,
  onChange
}: {
  value: RepeatSchedule;
  disabled: boolean;
  disableNoRepeat: boolean;
  autoOpen: boolean;
  onChange: (value: RepeatSchedule) => void;
}): JSX.Element {
  const [open, setOpen] = useState(autoOpen);
  const pickerRef = useRef<HTMLDivElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const schedule = normalizeRepeatSchedule(value);
  const interval = schedule.type === 'none' ? 1 : schedule.interval;
  const unit = repeatScheduleUnit(schedule.type);

  useEffect(() => {
    if (!open) return undefined;
    const dismissOnOutsidePointer = (event: PointerEvent): void => {
      if (pickerRef.current?.contains(event.target as Node)) return;
      setOpen(false);
    };
    const dismissOnEscape = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('pointerdown', dismissOnOutsidePointer);
    document.addEventListener('keydown', dismissOnEscape);
    return () => {
      document.removeEventListener('pointerdown', dismissOnOutsidePointer);
      document.removeEventListener('keydown', dismissOnEscape);
    };
  }, [open]);

  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  useLayoutEffect(() => {
    if (open && autoOpen) menuRef.current?.focus();
  }, [open, autoOpen]);

  const selectType = (type: RepeatSchedule['type']): void => {
    if (disableNoRepeat && type === 'none') return;
    onChange(repeatScheduleFor(type, type === schedule.type ? interval : 1));
  };

  const selectInterval = (nextInterval: number): void => {
    onChange(repeatScheduleFor(schedule.type === 'none' ? 'daily' : schedule.type, nextInterval));
  };

  const selectUnit = (nextUnit: RepeatScheduleUnit): void => {
    const type = repeatScheduleTypeForUnit(nextUnit);
    onChange(repeatScheduleFor(type, interval));
  };

  return (
    <div
      className={`new-research-repeat-picker${schedule.type !== 'none' ? ' is-non-default' : ''}${open ? ' is-open' : ''}`}
      ref={pickerRef}
      onBlur={(event) => {
        if (event.currentTarget.contains(event.relatedTarget)) return;
        setOpen(false);
      }}
    >
      <button
        type="button"
        className="new-research-repeat-trigger"
        title="Repeat schedule"
        aria-label="Repeat schedule"
        aria-haspopup="dialog"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={(event) => {
          if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
          event.preventDefault();
          setOpen(true);
        }}
      >
        <Repeat size={13} aria-hidden="true" />
        <span>{repeatScheduleLabel(schedule)}</span>
        <ChevronDown size={13} aria-hidden="true" />
      </button>
      {open ? (
        <div className="new-research-repeat-menu" role="dialog" aria-label="Repeat schedule" tabIndex={-1} ref={menuRef}>
          <div className="new-research-repeat-presets" role="listbox" aria-label="Repeat preset">
            {REPEAT_SCHEDULE_TYPES.map((type) => (
              <button
                type="button"
                role="option"
                aria-selected={schedule.type === type}
                className={schedule.type === type ? 'is-selected' : undefined}
                disabled={disableNoRepeat && type === 'none'}
                onClick={() => selectType(type)}
                key={type}
              >
                {repeatTypeLabel(type)}
              </button>
            ))}
          </div>
          <div className="new-research-repeat-widget">
            <label>
              <span>Every</span>
              <input
                type="number"
                min={1}
                max={99}
                step={1}
                value={interval}
                disabled={schedule.type === 'none'}
                onChange={(event) => selectInterval(Number(event.target.value))}
              />
            </label>
            <select
              value={unit}
              disabled={schedule.type === 'none'}
              aria-label="Repeat unit"
              onChange={(event) => selectUnit(event.target.value as RepeatScheduleUnit)}
            >
              <option value="minute">{interval === 1 ? 'minute' : 'minutes'}</option>
              <option value="hour">{interval === 1 ? 'hour' : 'hours'}</option>
              <option value="day">{interval === 1 ? 'day' : 'days'}</option>
              <option value="week">{interval === 1 ? 'week' : 'weeks'}</option>
              <option value="month">{interval === 1 ? 'month' : 'months'}</option>
            </select>
          </div>
        </div>
      ) : null}
    </div>
  );
}

export function ResearchGoalChooser({
  workflows = LEGACY_RESEARCH_GOAL_WORKFLOWS,
  suggestions,
  loading,
  errors,
  selectedWorkflowId,
  onSelectWorkflow,
  onLoad,
  onSelect,
  onRetry
}: ResearchGoalChooserProps): JSX.Element {
  const activeWorkflow = workflows.find((workflow) => workflow.id === selectedWorkflowId)
    ?? workflows.find((workflow) => workflow.default)
    ?? workflows[0]
    ?? LEGACY_RESEARCH_GOAL_WORKFLOWS[0]!;
  const phase = activeWorkflow.id;
  const workflowIndex = Math.max(0, workflows.findIndex((workflow) => workflow.id === phase));
  const domId = workflowDomId(phase, workflowIndex);

  useEffect(() => {
    onLoad?.(phase);
  }, [onLoad, phase]);

  return (
    <section
      className="research-goal-chooser"
      aria-label="Research suggestions"
      aria-describedby={`research-goal-${domId}-description`}
    >
      <div className="research-goal-view-toggle" role="tablist" aria-label="Suggestion lanes">
        {workflows.map((workflow) => (
          <button
            type="button"
            role="tab"
            aria-selected={workflow.id === phase}
            className={workflow.id === phase ? 'selected' : undefined}
            onClick={() => onSelectWorkflow?.(workflow.id)}
            key={workflow.id}
          >
            {workflow.name}
          </button>
        ))}
      </div>
      <header className="research-goal-section-header">
        <div className="research-goal-description-row">
          <p id={`research-goal-${domId}-description`}>{activeWorkflow.description}</p>
          {loading[phase] ? <span role="status">Loading…</span> : null}
        </div>
      </header>
      <MainSideScrollRegion
        className="research-goal-choice-scroll"
        listClassName="research-goal-choice-list"
        stickToStart
        updateKey={`${phase}:${loading[phase] ? 'loading' : 'ready'}:${errors[phase] ?? ''}:${suggestions[phase]?.length ?? 0}`}
      >
        {errors[phase] ? (
          <div className="research-goal-section-error" role="alert">
            <ShieldAlert size={14} />
            <div>
              <strong>Could not load {activeWorkflow.name.toLowerCase()} goals</strong>
              <p>{errors[phase]}</p>
              <button type="button" onClick={() => onRetry(phase)}>
                <RefreshCw size={13} />
                Retry
              </button>
            </div>
          </div>
        ) : null}
        {suggestions[phase]?.slice(0, MAX_RENDERED_GOAL_SUGGESTIONS).map((sentence, index) => (
          <button
            type="button"
            className="research-goal-choice"
            aria-label={`${activeWorkflow.name} goal ${index + 1}: ${sentence}`}
            onClick={() => onSelect(sentence, phase)}
            key={sentence}
          >
            <span className="research-goal-choice-text">{sentence}</span>
          </button>
        ))}
        {loading[phase] ? Array.from({
          length: Math.max(0, Math.min(MAX_RENDERED_GOAL_SUGGESTIONS, Math.max(1, activeWorkflow.goalSuggestionCount))
            - (suggestions[phase]?.length ?? 0))
        }, (_, index) => index).map((index) => (
          <div className="research-goal-choice research-goal-choice-loading" aria-hidden="true" key={`loading-${index}`}>
            <span />
            <span />
          </div>
        )) : null}
      </MainSideScrollRegion>
    </section>
  );
}

function NewResearchWelcome({
  workspaceName,
  workflows,
  suggestions,
  loading,
  errors,
  visible,
  onOpenWorkflow,
  onSelect,
  onRegenerate
}: {
  workspaceName: string;
  workflows: readonly ResearchProfileWorkflow[];
  suggestions: ResearchGoalSuggestionsByPhase;
  loading: ResearchGoalSuggestionStateByPhase<boolean>;
  errors: ResearchGoalSuggestionStateByPhase<string | null>;
  visible: boolean;
  onOpenWorkflow: (workflowId: ResearchGoalPhase) => void;
  onSelect: (sentence: string, workflowId: ResearchGoalPhase) => void;
  onRegenerate: (workflowId: ResearchGoalPhase) => void;
}): JSX.Element {
  const [selectedWorkflowId, setSelectedWorkflowId] = useState<ResearchGoalPhase | null>(null);
  const selectedWorkflow = workflows.find((workflow) => workflow.id === selectedWorkflowId) ?? null;
  const suggestionLimit = selectedWorkflow
    ? Math.min(MAX_RENDERED_GOAL_SUGGESTIONS, Math.max(1, selectedWorkflow.goalSuggestionCount))
    : 3;

  return (
    <section className="new-research-welcome" aria-label={`New research for ${workspaceName}`}>
      <BealeWelcomeIcon />
      <h2>Let&apos;s research {workspaceName}</h2>
      {visible ? (
        selectedWorkflow ? (
          <div className="new-research-suggestion-panel">
            <SessionNextStepsWidget
              loading={loading[selectedWorkflow.id] ?? false}
              suggestions={suggestions[selectedWorkflow.id] ?? []}
              error={errors[selectedWorkflow.id] ?? null}
              title={null}
              suggestionLimit={suggestionLimit}
              onBack={() => setSelectedWorkflowId(null)}
              onRegenerate={() => onRegenerate(selectedWorkflow.id)}
              onSelect={(sentence) => onSelect(sentence, selectedWorkflow.id)}
            />
          </div>
        ) : (
          <div className="new-research-workflow-list" aria-label="Research suggestion categories">
            {workflows.map((workflow) => (
              <button
                type="button"
                className="new-research-workflow-option"
                aria-label={`${workflow.name}: ${workflow.description}`}
                onClick={() => {
                  setSelectedWorkflowId(workflow.id);
                  onOpenWorkflow(workflow.id);
                }}
                key={workflow.id}
              >
                <ResearchWorkflowIcon workflow={workflow} />
                <span>
                  <strong>{workflow.name}</strong>
                  <span>{workflow.description}</span>
                </span>
                <ArrowRight className="new-research-workflow-arrow" size={14} aria-hidden="true" />
              </button>
            ))}
          </div>
        )
      ) : null}
    </section>
  );
}

function ResearchWorkflowIcon({ workflow }: { workflow: ResearchProfileWorkflow }): JSX.Element {
  const identity = `${workflow.id} ${workflow.name}`.toLowerCase();
  if (identity.includes('discover')) return <Compass size={19} aria-hidden="true" />;
  if (identity.includes('chain')) return <Waypoints size={19} aria-hidden="true" />;
  if (identity.includes('report')) return <FileText size={19} aria-hidden="true" />;
  if (identity.includes('longshot')) return <Telescope size={19} aria-hidden="true" />;
  return <Lightbulb size={19} aria-hidden="true" />;
}

export function newResearchTargetSummary(names: readonly string[]): string {
  const first = names[0];
  if (!first) return 'No target';
  return names.length > 1 ? `${first} +${names.length - 1}` : first;
}

export function toggleResearchTargetSelection(selected: readonly string[], targetId: string): string[] {
  return selected.includes(targetId)
    ? selected.filter((id) => id !== targetId)
    : selected.length < 16 ? [...selected, targetId] : [...selected];
}

function MachinePicker({ targets, values, disabled, error, onChange, onRetry }: {
  targets: FleetTargetChoice[] | null;
  values: readonly string[];
  disabled: boolean;
  error: string | null;
  onChange: (machineId: string) => void;
  onRetry: () => void;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const pickerRef = useRef<HTMLDivElement | null>(null);
  const selected = values.map((id) => targets?.find((target) => target.id === id)).filter((target) => target !== undefined);
  const ready = selected.length > 0 && selected.length === values.length && selected.every((target) => target.ready);
  const retry = !targets && Boolean(error);
  const label = !targets ? error ? 'Targets unavailable' : 'Loading…' : newResearchTargetSummary(selected.map((target) => target.name));

  useEffect(() => {
    if (!open) return undefined;
    const dismissOnOutsidePointer = (event: PointerEvent): void => {
      if (!pickerRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const dismissOnEscape = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('pointerdown', dismissOnOutsidePointer);
    document.addEventListener('keydown', dismissOnEscape);
    return () => {
      document.removeEventListener('pointerdown', dismissOnOutsidePointer);
      document.removeEventListener('keydown', dismissOnEscape);
    };
  }, [open]);

  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);

  const chooseMachine = (machineId: string): void => {
    onChange(machineId);
  };

  return <div className={`new-research-machine-picker${open ? ' is-open' : ''}`} ref={pickerRef}>
    <button type="button" className="new-research-machine-trigger" title={error ? `${error} Click to retry.` : 'Choose workspace targets for this research session.'}
      aria-label={retry ? 'Retry loading research targets' : `Research targets: ${label}${ready ? ', ready' : ', not ready'}`}
      aria-haspopup={retry ? undefined : 'dialog'} aria-expanded={retry ? undefined : open}
      disabled={disabled || (!targets && !retry) || targets?.length === 0}
      onClick={() => retry ? onRetry() : setOpen((current) => !current)}>
      <span className={`new-research-machine-status${ready ? ' is-ready' : ' is-unready'}`} aria-hidden="true" />
      <strong>{label}</strong>{retry ? <RefreshCw size={13} aria-hidden="true" /> : <ChevronDown size={13} aria-hidden="true" />}
    </button>
    {open ? <div className="new-research-machine-menu" role="dialog" aria-label="Research targets">
      {targets?.map((target) => <label className={`new-research-machine-option${values.includes(target.id) ? ' is-selected' : ''}`}
        title={!values.includes(target.id) && values.length >= 16 ? 'Select up to 16 targets.'
          : target.ready ? target.detail : 'Configure this target in Fleet'} key={target.id}>
        <input type="checkbox" checked={values.includes(target.id)} disabled={!values.includes(target.id) && values.length >= 16}
          onChange={() => chooseMachine(target.id)} />
        <span className={`new-research-machine-status${target.ready ? ' is-ready' : ' is-unready'}`} aria-hidden="true" />
        <span>{target.name}</span>
      </label>)}
      {error ? <p className="new-research-machine-menu-status is-error" role="alert" title={error}>Targets unavailable.</p> : null}
    </div> : null}
  </div>;
}

export function defaultResearchWorkflowId(workflows: readonly ResearchProfileWorkflow[]): string {
  return workflows.find((workflow) => workflow.id === 'discovery')?.id
    ?? workflows.find((workflow) => workflow.default)?.id
    ?? workflows[0]?.id
    ?? 'discovery';
}

export function researchSettingsInput(
  initialInput: StartRunInput | undefined,
  initialWorkflowId: string,
  initialGoal: ResearchGoalSeed | null,
  defaultShellSafetyMode: ShellSafetyMode = DEFAULT_SHELL_SAFETY_MODE
): StartRunInput {
  if (initialInput) {
    return {
      ...defaultRunInput,
      ...initialInput,
      provider: initialInput.provider,
      workflowId: initialInput.workflowId && initialInput.workflowId !== 'research'
        ? initialInput.workflowId
        : initialWorkflowId,
      collaboration: initialInput.collaboration
        ? normalizeResearchCollaboration(initialInput.collaboration)
        : normalizeResearchCollaboration(defaultRunInput.collaboration),
      budget: {
        ...defaultRunInput.budget,
        ...initialInput.budget,
        repeatSchedule: normalizeRepeatSchedule(initialInput.budget.repeatSchedule)
      }
    };
  }
  return {
    ...defaultRunInput,
    shellSafetyMode: defaultShellSafetyMode,
    workflowId: initialWorkflowId,
    goalObjective: initialGoal?.sentence ?? null,
    promptMarkdown: initialGoal?.promptMarkdown ?? initialGoal?.sentence ?? '',
    sandboxProfile: 'host',
    budget: { ...defaultRunInput.budget },
    collaboration: normalizeResearchCollaboration(defaultRunInput.collaboration)
  };
}

function researchProviderId(value: string | null | undefined): ResearchModelProviderId | null {
  if (value === 'openai-codex' || value === 'anthropic' || value === 'xai' || value === 'zai' || value === 'openrouter') return value;
  if (value && /^custom-[a-z0-9][a-z0-9-]{0,63}$/u.test(value)) return value as `custom-${string}`;
  return null;
}

function providerPolicyReady(providerId: ResearchModelProviderId, acknowledgements: ProviderSettings['cyberPolicyRiskAcknowledgements']): boolean {
  return providerId.startsWith('custom-') || acknowledgements?.[providerId] === true;
}

function repeatTypeLabel(type: RepeatSchedule['type']): string {
  if (type === 'none') return 'No Repeat';
  if (type === 'minutely') return 'Every minute';
  if (type === 'hourly') return 'Hourly';
  if (type === 'daily') return 'Daily';
  if (type === 'weekly') return 'Weekly';
  return 'Monthly';
}

function repeatScheduleUnit(type: RepeatSchedule['type']): RepeatScheduleUnit {
  if (type === 'minutely') return 'minute';
  if (type === 'hourly') return 'hour';
  if (type === 'weekly') return 'week';
  if (type === 'monthly') return 'month';
  return 'day';
}

function repeatScheduleTypeForUnit(unit: RepeatScheduleUnit): Exclude<RepeatSchedule['type'], 'none'> {
  if (unit === 'minute') return 'minutely';
  if (unit === 'hour') return 'hourly';
  if (unit === 'week') return 'weekly';
  if (unit === 'month') return 'monthly';
  return 'daily';
}

function workflowDomId(id: string, index: number): string {
  const normalized = id.trim().toLocaleLowerCase().replace(/[^a-z0-9_-]+/gu, '-').replace(/^-+|-+$/gu, '');
  return normalized || `workflow-${index + 1}`;
}

export function collaborationRequiresCyberPolicyAcknowledgement(profileId: string | null | undefined): boolean {
  return profileId === 'security-research';
}

function collaboratorKey(providerId: ResearchModelProviderId, modelId: string): string {
  return `${providerId}\u0000${modelId}`;
}

export function selectedSessionProviderIds(input: StartRunInput): ResearchModelProviderId[] {
  const providerIds = new Set<ResearchModelProviderId>();
  const leadProvider = researchProviderId(input.provider);
  if (leadProvider) providerIds.add(leadProvider);
  const collaboration = normalizeResearchCollaboration(input.collaboration);
  for (const collaborator of collaboration.providers) {
    if (collaborator.enabled) providerIds.add(collaborator.provider);
  }
  return [...providerIds];
}

function formatProviderNames(names: readonly string[]): string {
  if (names.length === 0) return 'the selected provider';
  if (names.length === 1) return names[0]!;
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names.slice(0, -1).join(', ')}, and ${names.at(-1)}`;
}

export function selectNextAvailableCollaborator<T extends { provider: ResearchModelProviderId }>(
  available: readonly T[],
  enabled: readonly { provider: ResearchModelProviderId }[],
  leadProviderId: ResearchModelProviderId | null
): T | null {
  const representedProviders = new Set<ResearchModelProviderId>(enabled.map((collaborator) => collaborator.provider));
  if (leadProviderId) representedProviders.add(leadProviderId);
  return available.find((candidate) => !representedProviders.has(candidate.provider))
    ?? available.find((candidate) => candidate.provider !== leadProviderId)
    ?? available[0]
    ?? null;
}

function providerLabel(providerId: ResearchModelProviderId, fallback: string): string {
  if (providerId === 'openai-codex') return 'OpenAI (Codex)';
  if (providerId === 'anthropic') return 'Anthropic (Claude)';
  if (providerId === 'xai') return 'xAI (Grok/X)';
  return fallback;
}

function providerDefaultModel(
  providerId: ResearchModelProviderId,
  openAiStatus: OpenAiAccountStatus | null,
  statuses: ResearchProviderStatus[],
  modelDefaults: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>>
): string | null {
  const configuredDefault = modelDefaults[providerId]?.largeModel;
  if (configuredDefault) return configuredDefault;
  if (providerId === 'openai-codex') return openAiStatus?.defaultModel ?? DEFAULT_RESEARCH_MODEL;
  return statuses.find((provider) => provider.id === providerId)?.defaultModel ?? null;
}

function effortLevelFromInput(value: string): ResearchModelEffortLevel {
  return value.trim() ? value as ResearchModelEffortLevel : 'off';
}

function inputValueForEffort(value: ResearchModelEffortLevel): string {
  return value === 'off' ? '' : value;
}

function preferredEffort(levels: ResearchModelEffortLevel[], current: ResearchModelEffortLevel): ResearchModelEffortLevel {
  if (levels.includes(current)) return current;
  if (levels.includes('high')) return 'high';
  return levels[0] ?? 'off';
}

function effortLabel(effort: ResearchModelEffortLevel): string {
  if (effort === 'xhigh') return 'XHigh';
  return `${effort.slice(0, 1).toUpperCase()}${effort.slice(1)}`;
}
