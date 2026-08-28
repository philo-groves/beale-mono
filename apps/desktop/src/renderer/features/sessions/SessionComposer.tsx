import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { JSX, ReactNode } from 'react';
import { ArrowRight, ChevronDown, Plus, Shield, Square, Users, X } from 'lucide-react';
import type {
  ApprovalRecord,
  PolicyReviewDecision,
  ProviderModelDefaults,
  ResearchCollaborationPreferences,
  ResearchCollaborationProviderPreference,
  ResearchModelEffortLevel,
  ResearchModelProviderId,
  ResearchModelSelection,
  ResearchSubagentMode,
  ResearchSubagentRole,
  ResearchProviderModel,
  ResearchProviderModelCatalog,
  RunDetail,
  ShellSafetyMode,
  SteeringAction
} from '@shared/types';
import { ModelSelectionPicker } from '../../app/ModelSelectionPicker';
import { Modal } from '../../app/Modal';
import { FloatingTextPicker } from '../../app/FloatingTextPicker';
import { CenteredLoadingState } from '../../app/CenteredLoadingState';
import { researchModelNameLabel } from '../../lib/formatting';
import { normalizeShellSafetyMode, shellSafetyModeLabel, SHELL_SAFETY_MODE_OPTIONS } from '../../../shared/shellSafety';
import { normalizeResearchCollaboration, RESEARCH_SUBAGENT_ROLES } from '../../../shared/collaboration';
export { SHELL_SAFETY_MODE_OPTIONS } from '../../../shared/shellSafety';
import { contextMeterForDetail, visibleContextWindowPercentageLabel } from '../momentum/contextMeter';
import {
  steeringInputSuggestion,
  steeringInputTabAction,
  steeringSuggestionAutoVisible
} from '../../view-models/steeringSuggestions';
import { ShellApprovalQuestion } from './ShellApprovalModal';

export const STEER_TEXTAREA_MAX_LINES = 7;
export const STEER_TEXTAREA_DEFAULT_EXTRA_LINES = 1;
const STEER_ACTION_ROW_HEIGHT = 35;
const STEER_COMPOSER_ROW_GAP = 0;

export function SessionLoadingState({ label }: { label: string }): JSX.Element {
  return <CenteredLoadingState className="main-session-loading" label={label} />;
}

export const MainSteerArea = memo(function MainSteerArea({
  runId,
  detail,
  providerModelCatalog,
  busy,
  shellApproval = null,
  shellApprovalBusy = false,
  initialModelSelection,
  providerModelDefaults,
  collaboration: collaborationInput,
  initialSafetyMode,
  initialInstruction = '',
  initialSuggestion,
  inputPlaceholder,
  dangerModeEnabled = false,
  safetyModeOptions = SHELL_SAFETY_MODE_OPTIONS,
  preComposerContent,
  postComposerContent,
  ariaLabel = 'Steer research session',
  showCollaboration = true,
  showSafetyMode = true,
  responseSuggestionsEnabled = true,
  onCollaborationChange,
  onInitialInstruction,
  onCancel,
  onShellApprovalDecision = () => undefined,
  onSessionAction,
  onSteerInstruction
}: {
  runId: string | null;
  detail: RunDetail | null;
  providerModelCatalog: ResearchProviderModelCatalog[];
  busy: boolean;
  shellApproval?: ApprovalRecord | null;
  shellApprovalBusy?: boolean;
  initialModelSelection?: ResearchModelSelection;
  providerModelDefaults?: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>>;
  collaboration?: ResearchCollaborationPreferences;
  initialSafetyMode?: ShellSafetyMode;
  initialInstruction?: string;
  initialSuggestion?: string;
  inputPlaceholder?: string;
  dangerModeEnabled?: boolean;
  safetyModeOptions?: Array<{ value: ShellSafetyMode; label: string }>;
  preComposerContent?: ReactNode;
  postComposerContent?: ReactNode;
  ariaLabel?: string;
  showCollaboration?: boolean;
  showSafetyMode?: boolean;
  responseSuggestionsEnabled?: boolean;
  onCollaborationChange?: (collaboration: ResearchCollaborationPreferences) => void;
  onInitialInstruction?: (
    instruction: string,
    modelSelection: ResearchModelSelection,
    shellSafetyMode: ShellSafetyMode
  ) => void;
  onCancel?: () => void;
  onShellApprovalDecision?: (decision: PolicyReviewDecision) => void;
  onSessionAction: (action: SteeringAction) => void;
  onSteerInstruction: (runId: string, instruction: string, modelSelection: ResearchModelSelection) => void;
}): JSX.Element {
  const [instruction, setInstruction] = useState(initialInstruction);
  const [tabSuggestionVisible, setTabSuggestionVisible] = useState(false);
  const runProviderId = runModelProvider(detail, providerModelCatalog);
  const initialProviderId = detail ? runProviderId : initialModelSelection?.provider ?? runProviderId;
  const initialProvider = providerModelCatalog.find((catalog) => catalog.providerId === initialProviderId)
    ?? providerModelCatalog.find((catalog) => catalog.models.length > 0)
    ?? null;
  const initialModel = initialProvider?.models.find((model) => model.id === initialModelSelection?.model)
    ?? initialProvider?.models[0]
    ?? null;
  const [selectedProviderId, setSelectedProviderId] = useState<ResearchModelProviderId>(initialProvider?.providerId ?? runProviderId);
  const [selectedModelId, setSelectedModelId] = useState(detail?.run.model ?? initialModel?.id ?? '');
  const [selectedEffort, setSelectedEffort] = useState<ResearchModelEffortLevel>(() => detail
    ? researchEffort(detail.run.reasoningEffort)
    : preferredResearchEffort(initialModel?.effortLevels ?? [], initialModelSelection?.reasoningEffort ?? 'high'));
  const [initialShellSafetyMode, setInitialShellSafetyMode] = useState<ShellSafetyMode>(() =>
    normalizeShellSafetyMode(detail?.run.shellSafetyMode ?? initialSafetyMode)
  );
  const footerRef = useRef<HTMLElement | null>(null);
  const preComposerRef = useRef<HTMLDivElement | null>(null);
  const postComposerRef = useRef<HTMLDivElement | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const focusedRunIdRef = useRef<string | null>(null);
  const trimmedInstruction = instruction.trim();
  const disabled = busy || (!runId && !onInitialInstruction) || !trimmedInstruction || !selectedModelId;
  const status = detail?.run.status ?? null;
  const steeringSuggestion = responseSuggestionsEnabled
    ? initialSuggestion ?? steeringInputSuggestion(detail)
    : null;
  const suggestionShowing = Boolean(
    steeringSuggestion && (initialSuggestion || tabSuggestionVisible || steeringSuggestionAutoVisible(status))
  );
  const shellSafetyMode = detail
    ? normalizeShellSafetyMode(detail.run.shellSafetyMode)
    : initialShellSafetyMode;
  const availableSafetyModeOptions = steeringSafetyModeOptions(safetyModeOptions, dangerModeEnabled);
  const selectedSafetyModeLabel = availableSafetyModeOptions.some((option) => option.value === shellSafetyMode)
    ? undefined
    : shellSafetyModeLabel(shellSafetyMode);
  const sessionControlsDisabled = busy || !runId;
  const composerControlsDisabled = busy || (!runId && !onInitialInstruction);
  const fallbackModel = detail ? fallbackResearchModel(detail.run.model, researchEffort(detail.run.reasoningEffort)) : null;
  const providerOptions = detail && !providerModelCatalog.some((catalog) => catalog.providerId === runProviderId)
    ? [
        ...providerModelCatalog,
        {
          providerId: runProviderId,
          providerName: researchProviderLabel(runProviderId, runProviderId),
          models: fallbackModel ? [fallbackModel] : []
        }
      ]
    : providerModelCatalog;
  const providerCatalog = providerOptions.find((catalog) => catalog.providerId === selectedProviderId) ?? null;
  const modelOptions = providerCatalog?.models.length
    ? providerCatalog.models
    : fallbackModel && selectedProviderId === runProviderId ? [fallbackModel] : [];
  const selectedModel = modelOptions.find((model) => model.id === selectedModelId) ?? modelOptions[0] ?? null;
  const modelSelection: ResearchModelSelection = {
    provider: selectedProviderId,
    model: selectedModel?.id ?? detail?.run.model ?? '',
    reasoningEffort: selectedEffort
  };
  const collaboration = normalizeResearchCollaboration(collaborationInput ?? detail?.run.budget.collaboration);

  useEffect(() => {
    if (!detail) {
      const nextProvider = providerModelCatalog.find((catalog) => catalog.providerId === initialModelSelection?.provider)
        ?? providerModelCatalog.find((catalog) => catalog.models.length > 0);
      const nextModel = nextProvider?.models.find((model) => model.id === initialModelSelection?.model)
        ?? nextProvider?.models[0];
      if (!nextProvider || !nextModel) return;
      setSelectedProviderId(nextProvider.providerId);
      setSelectedModelId(nextModel.id);
      setSelectedEffort((current) => preferredResearchEffort(
        nextModel.effortLevels,
        initialModelSelection?.reasoningEffort ?? (current === 'off' ? 'high' : current)
      ));
      return;
    }
    const nextModel = providerModelCatalog
      .find((catalog) => catalog.providerId === runModelProvider(detail, providerModelCatalog))
      ?.models.find((model) => model.id === detail.run.model);
    const nextEffort = preferredResearchEffort(
      nextModel?.effortLevels ?? [researchEffort(detail.run.reasoningEffort)],
      researchEffort(detail.run.reasoningEffort)
    );
    setSelectedProviderId(runModelProvider(detail, providerModelCatalog));
    setSelectedModelId(nextModel?.id ?? detail.run.model);
    setSelectedEffort(nextEffort);
  }, [
    detail?.run.id,
    detail?.run.model,
    detail?.run.reasoningEffort,
    initialModelSelection?.model,
    initialModelSelection?.provider,
    initialModelSelection?.reasoningEffort,
    providerModelCatalog
  ]);

  useEffect(() => setTabSuggestionVisible(false), [runId, status, steeringSuggestion]);
  useEffect(() => {
    if (!detail) setInstruction(initialInstruction);
  }, [detail, initialInstruction]);

  const resizeTextarea = useCallback((): void => {
    const textarea = textareaRef.current;
    const footer = footerRef.current;
    if (!textarea || !footer) return;
    textarea.style.height = '0px';
    const computedStyle = window.getComputedStyle(textarea);
    const lineHeight = Number.parseFloat(computedStyle.lineHeight) || 16;
    const paddingTop = Number.parseFloat(computedStyle.paddingTop) || 0;
    const paddingBottom = Number.parseFloat(computedStyle.paddingBottom) || 0;
    const baseMinHeight = Number.parseFloat(computedStyle.minHeight) || 44;
    const minHeight = baseMinHeight + lineHeight * STEER_TEXTAREA_DEFAULT_EXTRA_LINES;
    const maxHeight = lineHeight * STEER_TEXTAREA_MAX_LINES + paddingTop + paddingBottom;
    const nextHeight = Math.max(minHeight, Math.min(textarea.scrollHeight, maxHeight));
    const accessoryOuterHeight = (element: HTMLDivElement | null): number => {
      if (!element) return 0;
      const style = window.getComputedStyle(element);
      const marginTop = Number.parseFloat(style.marginTop) || 0;
      const marginBottom = Number.parseFloat(style.marginBottom) || 0;
      return element.offsetHeight + marginTop + marginBottom;
    };
    const nextFooterHeight = accessoryOuterHeight(preComposerRef.current)
      + nextHeight
      + STEER_ACTION_ROW_HEIGHT
      + STEER_COMPOSER_ROW_GAP
      + accessoryOuterHeight(postComposerRef.current);
    textarea.style.height = `${nextHeight}px`;
    textarea.style.overflowY = textarea.scrollHeight > maxHeight ? 'auto' : 'hidden';
    const sessionView = footer.parentElement;
    sessionView?.style.removeProperty('--trace-footer-height');
    sessionView?.style.setProperty('--trace-footer-content-height', `${nextFooterHeight}px`);
  }, []);

  useLayoutEffect(() => resizeTextarea(), [instruction, postComposerContent, preComposerContent, resizeTextarea, shellApproval, status]);
  useEffect(() => {
    window.addEventListener('resize', resizeTextarea);
    return () => window.removeEventListener('resize', resizeTextarea);
  }, [resizeTextarea]);
  useEffect(() => {
    if (!runId || focusedRunIdRef.current === runId) return undefined;
    focusedRunIdRef.current = runId;
    const frame = window.requestAnimationFrame(() => textareaRef.current?.focus({ preventScroll: true }));
    return () => window.cancelAnimationFrame(frame);
  }, [runId]);

  const submit = (): void => {
    if (disabled) return;
    if (runId) onSteerInstruction(runId, trimmedInstruction, modelSelection);
    else onInitialInstruction?.(trimmedInstruction, modelSelection, shellSafetyMode);
    setInstruction('');
    setTabSuggestionVisible(false);
  };

  if (shellApproval) {
    return <ShellApprovalQuestion approval={shellApproval} busy={shellApprovalBusy} onDecision={onShellApprovalDecision} />;
  }

  const sessionActive = status === 'active';
  const placeholder = suggestionShowing && steeringSuggestion
    ? steeringSuggestion
    : inputPlaceholder ?? (sessionActive ? 'Steer the research' : 'Your move');

  return (
    <footer
      className={`main-trace-footer${preComposerContent ? ' has-pre-composer-content' : ''}${postComposerContent ? ' has-post-composer-content' : ''}`}
      ref={footerRef}
      aria-label={ariaLabel}
    >
      {preComposerContent ? (
        <div className="main-steer-pre-composer-content" ref={preComposerRef}>{preComposerContent}</div>
      ) : null}
      <div className={`main-steer-input-row without-trace-filters${showCollaboration ? '' : ' without-collaboration'}${showSafetyMode ? '' : ' without-safety-mode'}`}>
        <textarea
          ref={textareaRef}
          rows={1}
          value={instruction}
          placeholder={placeholder}
          onChange={(event) => {
            setInstruction(event.target.value);
            setTabSuggestionVisible(false);
          }}
          onKeyDown={(event) => {
            if (event.key === 'Escape' && onCancel) {
              event.preventDefault();
              onCancel();
              return;
            }
            if (event.key === 'Tab' && !event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey) {
              const action = steeringInputTabAction({ instruction, suggestion: steeringSuggestion, suggestionShowing });
              if (action !== 'none') {
                event.preventDefault();
                if (action === 'accept_suggestion' && steeringSuggestion) {
                  setInstruction(steeringSuggestion);
                  setTabSuggestionVisible(false);
                } else {
                  setTabSuggestionVisible(true);
                }
                return;
              }
            }
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              submit();
            }
          }}
        />
        <ModelSelectionPicker
          className="main-steer-model-selection-picker"
          providerValue={selectedProviderId}
          modelValue={selectedModel?.id ?? ''}
          effortValue={selectedEffort}
          title="Model settings for the next agent turn"
          ariaLabel="Model settings for the next agent turn"
          disabled={!selectedModel || composerControlsDisabled}
          providerOptions={providerOptions.map((provider) => ({
            value: provider.providerId,
            label: researchProviderLabel(provider.providerId, provider.providerName),
            disabled: provider.models.length === 0
          }))}
          modelOptions={modelOptions.map((model) => ({ value: model.id, label: researchModelNameLabel(selectedProviderId, model.name) }))}
          effortOptions={(selectedModel?.effortLevels ?? []).map((effort) => ({ value: effort, label: researchEffortLabel(effort) }))}
          onSelectProvider={(value) => {
            const providerId = value as ResearchModelProviderId;
            const nextProvider = providerOptions.find((provider) => provider.providerId === providerId);
            const nextModel = nextProvider?.models.find((model) => model.id === selectedModelId) ?? nextProvider?.models[0];
            if (!nextModel) return;
            setSelectedProviderId(providerId);
            setSelectedModelId(nextModel.id);
            setSelectedEffort((current) => preferredResearchEffort(nextModel.effortLevels, current));
          }}
          onSelectModel={(value) => {
            const model = modelOptions.find((candidate) => candidate.id === value);
            if (!model) return;
            setSelectedModelId(model.id);
            setSelectedEffort((current) => preferredResearchEffort(model.effortLevels, current));
          }}
          onSelectEffort={(value) => setSelectedEffort(value as ResearchModelEffortLevel)}
        />
        {showCollaboration ? (
          <CollaborationSelector
            collaboration={collaboration}
            disabled={composerControlsDisabled}
            leadModelSelection={modelSelection}
            providerModelDefaults={providerModelDefaults}
            onChange={onCollaborationChange ?? (runId ? (nextCollaboration) => onSessionAction({
              type: 'update_run_budget',
              runId,
              budgetPatch: { collaboration: nextCollaboration },
              note: 'Collaboration settings updated.'
            }) : undefined)}
            providerModelCatalog={providerOptions}
          />
        ) : null}
        {showSafetyMode ? (
          <FloatingTextPicker
            className={`main-steer-safety-mode-picker mode-${shellSafetyMode}`}
            leadingIcon={<Shield aria-hidden="true" className="main-steer-safety-mode-icon" size={13} />}
            value={shellSafetyMode}
            options={availableSafetyModeOptions}
            selectedLabelOverride={selectedSafetyModeLabel}
            title="Shell safety mode"
            ariaLabel="Shell safety mode"
            disabled={busy || status === 'paused' || (!runId && !onInitialInstruction)}
            onChange={(value) => {
              const nextMode = normalizeShellSafetyMode(value);
              if (nextMode === 'danger' && !dangerModeEnabled) return;
              if (nextMode === shellSafetyMode) return;
              if (!runId) setInitialShellSafetyMode(nextMode);
              else onSessionAction({ type: 'set_shell_safety_mode', runId, shellSafetyMode: nextMode });
            }}
          />
        ) : null}
        <ContextUsageDonut detail={detail} />
        {sessionActive ? (
          <button
            type="button"
            className="main-steer-send main-steer-stop"
            title="Stop session"
            aria-label="Stop session"
            disabled={sessionControlsDisabled}
            onClick={() => runId && onSessionAction({ type: 'stop', runId, note: 'Stop requested from session composer.' })}
          >
            <Square size={11} fill="currentColor" />
          </button>
        ) : (
          <button type="button" className="main-steer-send" title="Send steering instruction" aria-label="Send steering instruction" disabled={disabled} onClick={submit}>
            <ArrowRight size={16} />
          </button>
        )}
      </div>
      {postComposerContent ? (
        <div className="main-steer-post-composer-content" ref={postComposerRef}>{postComposerContent}</div>
      ) : null}
    </footer>
  );
});

function ContextUsageDonut({ detail }: { detail: RunDetail | null }): JSX.Element {
  const contextMeter = contextMeterForDetail(detail);
  const percentage = contextMeter.fraction * 100;
  const percentageLabel = visibleContextWindowPercentageLabel(contextMeter);
  return (
    <div
      className="main-steer-context-usage"
      role="progressbar"
      aria-label={`Context usage: ${percentageLabel}`}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(percentage)}
      title={`${percentageLabel} context used (${contextMeter.label})`}
    >
      <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
        <circle className="main-steer-context-usage-track" cx="8" cy="8" r="6" pathLength="100" />
        <circle
          className="main-steer-context-usage-value"
          cx="8"
          cy="8"
          r="6"
          pathLength="100"
          strokeDasharray={`${percentage} ${100 - percentage}`}
        />
      </svg>
    </div>
  );
}

function CollaborationSelector({
  collaboration,
  disabled,
  leadModelSelection,
  providerModelDefaults,
  onChange,
  providerModelCatalog
}: {
  collaboration: ResearchCollaborationPreferences;
  disabled: boolean;
  leadModelSelection: ResearchModelSelection;
  providerModelDefaults?: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>>;
  onChange?: (collaboration: ResearchCollaborationPreferences) => void;
  providerModelCatalog: ResearchProviderModelCatalog[];
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(collaboration);
  const collaborators = collaboration.providers.filter((provider) => provider.enabled);
  const isSelfCollaboration = collaborators.length === 1
    && collaborators[0]?.provider === leadModelSelection.provider
    && collaborators[0]?.model === leadModelSelection.model;
  const collaboratorLabel = isSelfCollaboration
    ? null
    : `${collaborators.length} Collabs`;
  const modeLabel = isSelfCollaboration
    ? 'Self-Collab'
    : collaboration.subagentMode === 'advanced' ? 'Advanced' : 'Simple';
  const openDialog = (): void => {
    setDraft(collaboration);
    setOpen(true);
  };
  const changeDraft = (next: ResearchCollaborationPreferences): void => {
    setDraft(next);
    onChange?.(next);
  };
  return (
    <>
      <div className={`main-steer-collaboration-selector${open ? ' is-open' : ''}`}>
        <button
          type="button"
          className="main-steer-collaboration-trigger"
          title="Collaboration settings"
          aria-label="Collaboration settings"
          aria-haspopup="dialog"
          aria-expanded={open}
          disabled={disabled}
          onClick={openDialog}
        >
          <Users className="main-steer-collaboration-icon" size={13} aria-hidden="true" />
          {collaboratorLabel ? <span className="main-steer-collaboration-label">{collaboratorLabel}</span> : null}
          <span className="main-steer-collaboration-mode">{modeLabel}</span>
          <ChevronDown className="main-steer-collaboration-chevron" size={13} aria-hidden="true" />
        </button>
      </div>
      {open ? (
        <Modal
          className="collaboration-selector-dialog"
          title="Collaboration"
          onClose={() => setOpen(false)}
        >
          <CollaborationSettingsForm
            collaboration={draft}
            disabled={!onChange}
            leadModelSelection={leadModelSelection}
            providerModelDefaults={providerModelDefaults}
            providerModelCatalog={providerModelCatalog}
            onChange={changeDraft}
          />
        </Modal>
      ) : null}
    </>
  );
}

const SUBAGENT_ROLE_OPTIONS: ReadonlyArray<{ value: ResearchSubagentRole; label: string }> = [
  { value: 'discoverer', label: 'Discoverer' },
  { value: 'prover', label: 'Prover' },
  { value: 'reviewer', label: 'Reviewer' },
  { value: 'reporter', label: 'Reporter' }
];

function collaboratorRoles(preference: ResearchCollaborationProviderPreference): ResearchSubagentRole[] {
  return preference.roles?.length ? preference.roles : [...RESEARCH_SUBAGENT_ROLES];
}

export function CollaborationSettingsForm({
  collaboration,
  disabled,
  leadModelSelection,
  providerModelDefaults,
  providerModelCatalog,
  onChange
}: {
  collaboration: ResearchCollaborationPreferences;
  disabled: boolean;
  leadModelSelection: ResearchModelSelection;
  providerModelDefaults?: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>>;
  providerModelCatalog: ResearchProviderModelCatalog[];
  onChange: (collaboration: ResearchCollaborationPreferences) => void;
}): JSX.Element {
  const enabledCollaborators = collaboration.providers.filter((preference) => preference.enabled);
  const enabledKeys = new Set(enabledCollaborators.map(collaborationProviderKey));
  const nextCollaborator = selectNextCollaborationRoute(
    collaboration,
    providerModelCatalog,
    leadModelSelection.provider,
    providerModelDefaults
  );

  const updatePreference = (
    current: ResearchCollaborationProviderPreference,
    next: ResearchCollaborationProviderPreference
  ): void => {
    const currentKey = collaborationProviderKey(current);
    const nextKey = collaborationProviderKey(next);
    if (currentKey === nextKey) {
      onChange({
        ...collaboration,
        providers: collaboration.providers.map((preference) => collaborationProviderKey(preference) === currentKey
          ? { ...preference, ...next, enabled: true }
          : preference)
      });
      return;
    }
    const nextProviders = collaboration.providers
      .map((preference) => collaborationProviderKey(preference) === currentKey
        ? { ...preference, enabled: false }
        : collaborationProviderKey(preference) === nextKey
          ? { ...preference, ...next, enabled: true }
          : preference);
    if (!nextProviders.some((preference) => collaborationProviderKey(preference) === nextKey)) {
      nextProviders.unshift({ ...next, enabled: true });
    }
    onChange({ ...collaboration, providers: nextProviders });
  };

  const changeSubagentMode = (subagentMode: ResearchSubagentMode): void => {
    onChange({
      ...collaboration,
      subagentMode,
      providers: collaboration.providers.map((preference) => ({
        ...preference,
        roles: collaboratorRoles(preference)
      }))
    });
  };

  const addCollaborator = (): void => {
    if (!nextCollaborator) return;
    const reasoningEffort = nextCollaborator.existing?.reasoningEffort
      ?? preferredResearchEffort(nextCollaborator.model.effortLevels, leadModelSelection.reasoningEffort);
    const nextPreference: ResearchCollaborationProviderPreference = {
      provider: nextCollaborator.provider,
      model: nextCollaborator.model.id,
      reasoningEffort,
      enabled: true,
      roles: nextCollaborator.existing ? collaboratorRoles(nextCollaborator.existing) : [...RESEARCH_SUBAGENT_ROLES]
    };
    const nextKey = collaborationProviderKey(nextPreference);
    const exists = collaboration.providers.some((preference) => collaborationProviderKey(preference) === nextKey);
    onChange({
      ...collaboration,
      providers: exists
        ? collaboration.providers.map((preference) => collaborationProviderKey(preference) === nextKey
          ? { ...preference, ...nextPreference }
          : preference)
        : [nextPreference, ...collaboration.providers]
    });
  };

  const removeCollaborator = (preference: ResearchCollaborationProviderPreference): void => {
    if (enabledCollaborators.length <= 1) return;
    const key = collaborationProviderKey(preference);
    onChange({
      ...collaboration,
      providers: collaboration.providers.map((candidate) => collaborationProviderKey(candidate) === key
        ? { ...candidate, enabled: false }
        : candidate)
    });
  };

  return (
    <form className="settings-form collaboration-selector-form" aria-label="Collaboration settings" onSubmit={(event) => event.preventDefault()}>
      <fieldset className="settings-form-squircle" disabled={disabled}>
        <div className="settings-form-control-list collaboration-selector-form-list">
          <label className="settings-form-control-row collaboration-selector-mode-row">
            <span className="settings-form-control-copy">
              <strong>Subagent Mode</strong>
              <small>Choose direct collaborators or select their compatible Advanced roles.</small>
            </span>
            <select
              aria-label="Subagent mode"
              value={collaboration.subagentMode}
              onChange={(event) => changeSubagentMode(event.target.value as ResearchSubagentMode)}
            >
              <option value="simple">Simple</option>
              <option value="advanced">Advanced</option>
            </select>
          </label>
          {enabledCollaborators.map((preference, index) => {
            const catalog = providerModelCatalog.find((candidate) => candidate.providerId === preference.provider);
            const model = catalog?.models.find((candidate) => candidate.id === preference.model)
              ?? fallbackResearchModel(preference.model, preference.reasoningEffort);
            return (
              <div className="settings-form-control-row collaboration-selector-collaborator-row" key={collaborationProviderKey(preference)}>
                <span className="settings-form-control-copy">
                  <strong>Collaborator {index + 1}</strong>
                </span>
                <div className="collaboration-selector-row-controls">
                  {collaboration.subagentMode === 'advanced' ? (
                    <CollaborationRolePicker
                      collaboratorNumber={index + 1}
                      disabled={disabled}
                      roles={collaboratorRoles(preference)}
                      onChange={(roles) => updatePreference(preference, { ...preference, roles })}
                    />
                  ) : null}
                  <ModelSelectionPicker
                    className="collaboration-selector-model-picker"
                    providerValue={preference.provider}
                    modelValue={preference.model}
                    effortValue={preference.reasoningEffort}
                    title={`Collaborator ${index + 1} model settings`}
                    ariaLabel={`Collaborator ${index + 1} model settings`}
                    disabled={disabled}
                    providerOptions={providerModelCatalog.map((candidate) => ({
                      value: candidate.providerId,
                      label: researchProviderLabel(candidate.providerId, candidate.providerName),
                      disabled: candidate.providerId !== preference.provider && !candidate.models.some((candidateModel) => (
                        !enabledKeys.has(collaborationProviderKey({ provider: candidate.providerId, model: candidateModel.id }))
                      ))
                    }))}
                    modelOptions={(catalog?.models ?? [model]).map((candidate) => ({
                      value: candidate.id,
                      label: researchModelNameLabel(preference.provider, candidate.name),
                      disabled: candidate.id !== preference.model && enabledKeys.has(collaborationProviderKey({
                        provider: preference.provider,
                        model: candidate.id
                      }))
                    }))}
                    effortOptions={model.effortLevels.map((effort) => ({ value: effort, label: researchEffortLabel(effort) }))}
                    onSelectProvider={(providerId) => {
                      const nextCatalog = providerModelCatalog.find((candidate) => candidate.providerId === providerId);
                      const nextModel = (nextCatalog ? orderedCollaborationModels(
                        nextCatalog,
                        collaboration,
                        providerModelDefaults
                      ).find((candidate) => !enabledKeys.has(collaborationProviderKey({
                          provider: nextCatalog.providerId,
                          model: candidate.id
                        }))) : undefined) ?? (nextCatalog?.providerId === preference.provider
                        ? nextCatalog.models.find((candidate) => candidate.id === preference.model)
                        : undefined);
                      if (!nextCatalog || !nextModel) return;
                      const existing = collaboration.providers.find((candidate) => (
                        candidate.provider === nextCatalog.providerId && candidate.model === nextModel.id
                      ));
                      updatePreference(preference, {
                        provider: nextCatalog.providerId,
                        model: nextModel.id,
                        reasoningEffort: existing?.reasoningEffort
                          ?? preferredResearchEffort(nextModel.effortLevels, preference.reasoningEffort),
                        enabled: true,
                        roles: collaboratorRoles(preference)
                      });
                    }}
                    onSelectModel={(modelId) => {
                      const nextModel = catalog?.models.find((candidate) => candidate.id === modelId);
                      if (!nextModel) return;
                      const existing = collaboration.providers.find((candidate) => (
                        candidate.provider === preference.provider && candidate.model === modelId
                      ));
                      updatePreference(preference, {
                        provider: preference.provider,
                        model: modelId,
                        reasoningEffort: existing?.reasoningEffort
                          ?? preferredResearchEffort(nextModel.effortLevels, preference.reasoningEffort),
                        enabled: true,
                        roles: collaboratorRoles(preference)
                      });
                    }}
                    onSelectEffort={(effort) => updatePreference(preference, {
                      ...preference,
                      reasoningEffort: effort as ResearchModelEffortLevel
                    })}
                  />
                  {enabledCollaborators.length > 1 ? (
                    <button
                      type="button"
                      className="collaboration-selector-remove"
                      title={`Remove collaborator ${index + 1}`}
                      aria-label={`Remove collaborator ${index + 1}`}
                      onClick={() => removeCollaborator(preference)}
                    >
                      <X size={14} aria-hidden="true" />
                    </button>
                  ) : null}
                </div>
              </div>
            );
          })}
          <div className="settings-form-control-row collaboration-selector-add-row">
            <span className="settings-form-control-copy">
              <strong>Add Collaborator</strong>
              <small>{nextCollaborator ? 'Add another provider and model to this collaboration.' : 'No additional models are available.'}</small>
            </span>
            <button type="button" disabled={disabled || !nextCollaborator} onClick={addCollaborator}>
              <Plus size={14} aria-hidden="true" /> Add
            </button>
          </div>
        </div>
      </fieldset>
    </form>
  );
}

function CollaborationRolePicker({
  collaboratorNumber,
  disabled,
  roles,
  onChange
}: {
  collaboratorNumber: number;
  disabled: boolean;
  roles: ResearchSubagentRole[];
  onChange: (roles: ResearchSubagentRole[]) => void;
}): JSX.Element {
  const selected = new Set(roles);
  const label = roles.length === RESEARCH_SUBAGENT_ROLES.length
    ? 'All Roles'
    : SUBAGENT_ROLE_OPTIONS.filter((option) => selected.has(option.value)).map((option) => option.label).join(', ');
  const toggle = (role: ResearchSubagentRole): void => {
    if (selected.has(role)) {
      if (roles.length === 1) return;
      onChange(roles.filter((candidate) => candidate !== role));
      return;
    }
    onChange(RESEARCH_SUBAGENT_ROLES.filter((candidate) => selected.has(candidate) || candidate === role));
  };
  return (
    <details className="collaboration-selector-role-picker">
      <summary
        aria-label={`Collaborator ${collaboratorNumber} compatible roles`}
        aria-disabled={disabled}
        onClick={(event) => {
          if (disabled) event.preventDefault();
        }}
      >
        <span>{label}</span>
        <ChevronDown size={13} aria-hidden="true" />
      </summary>
      <div className="collaboration-selector-role-menu" role="group" aria-label={`Collaborator ${collaboratorNumber} compatible roles`}>
        {SUBAGENT_ROLE_OPTIONS.map((option) => {
          const checked = selected.has(option.value);
          return (
            <label key={option.value}>
              <input
                type="checkbox"
                checked={checked}
                disabled={disabled || (checked && roles.length === 1)}
                onChange={() => toggle(option.value)}
              />
              <span>{option.label}</span>
            </label>
          );
        })}
      </div>
    </details>
  );
}

function collaborationProviderKey(preference: { provider: string; model: string }): string {
  return `${preference.provider}\u0000${preference.model}`;
}

export interface CollaborationRouteCandidate {
  provider: ResearchModelProviderId;
  model: ResearchProviderModel;
  existing?: ResearchCollaborationProviderPreference;
}

export function selectNextCollaborationRoute(
  collaboration: ResearchCollaborationPreferences,
  providerModelCatalog: ResearchProviderModelCatalog[],
  leadProvider: ResearchModelProviderId,
  providerModelDefaults: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>> = {}
): CollaborationRouteCandidate | null {
  const enabledCollaborators = collaboration.providers.filter((preference) => preference.enabled);
  const enabledKeys = new Set(enabledCollaborators.map(collaborationProviderKey));
  const representedProviders = new Set(enabledCollaborators.map((preference) => preference.provider));
  representedProviders.add(leadProvider);
  const availableRoutes = providerModelCatalog.flatMap((catalog) => (
    orderedCollaborationModels(catalog, collaboration, providerModelDefaults).map((model) => ({
      provider: catalog.providerId,
      model,
      existing: collaboration.providers.find((preference) => (
        preference.provider === catalog.providerId && preference.model === model.id
      ))
    }))
  )).filter((route) => !enabledKeys.has(collaborationProviderKey({
    provider: route.provider,
    model: route.model.id
  })));
  return availableRoutes.find((route) => !representedProviders.has(route.provider))
    ?? availableRoutes[0]
    ?? null;
}

function orderedCollaborationModels(
  catalog: ResearchProviderModelCatalog,
  collaboration: ResearchCollaborationPreferences,
  providerModelDefaults: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>> = {}
): ResearchProviderModel[] {
  const preferredLargeModel = providerModelDefaults[catalog.providerId]?.largeModel;
  const savedOrder = new Map(collaboration.providers
    .filter((preference) => preference.provider === catalog.providerId)
    .map((preference, index) => [preference.model, index]));
  const rank = (model: ResearchProviderModel): number => {
    if (model.id === preferredLargeModel) return -1;
    return savedOrder.get(model.id) ?? Number.MAX_SAFE_INTEGER;
  };
  return [...catalog.models].sort((left, right) => rank(left) - rank(right));
}

export function steeringSafetyModeOptions(
  options: Array<{ value: ShellSafetyMode; label: string }>,
  dangerModeEnabled: boolean
): Array<{ value: ShellSafetyMode; label: string }> {
  return dangerModeEnabled ? options : options.filter((option) => option.value !== 'danger');
}

function runModelProvider(detail: RunDetail | null, catalogs: ResearchProviderModelCatalog[]): ResearchModelProviderId {
  const stored = detail?.run.budget.modelProvider;
  if (stored === 'openai-codex' || stored === 'anthropic' || stored === 'xai' || stored === 'zai' || stored === 'openrouter') return stored;
  const matchingCatalog = catalogs.find((catalog) => catalog.models.some((model) => model.id === detail?.run.model));
  return matchingCatalog?.providerId ?? 'openai-codex';
}

function researchEffort(value: string | undefined): ResearchModelEffortLevel {
  if (value === 'minimal' || value === 'low' || value === 'medium' || value === 'high' || value === 'xhigh' || value === 'max') return value;
  return 'off';
}

function preferredResearchEffort(levels: ResearchModelEffortLevel[], current: ResearchModelEffortLevel): ResearchModelEffortLevel {
  if (levels.includes(current)) return current;
  if (levels.includes('high')) return 'high';
  return levels[0] ?? 'off';
}

function fallbackResearchModel(model: string, effort: ResearchModelEffortLevel): ResearchProviderModel {
  return { id: model, name: model, reasoning: effort !== 'off', effortLevels: [effort], contextWindow: 0, maxTokens: 0 };
}

function researchProviderLabel(providerId: ResearchModelProviderId, fallback: string): string {
  if (providerId === 'openai-codex') return 'OpenAI (Codex)';
  if (providerId === 'anthropic') return 'Anthropic (Claude)';
  if (providerId === 'xai') return 'xAI (Grok/X)';
  return fallback;
}

function researchEffortLabel(effort: ResearchModelEffortLevel): string {
  if (effort === 'xhigh') return 'XHigh';
  return `${effort.slice(0, 1).toUpperCase()}${effort.slice(1)}`;
}
