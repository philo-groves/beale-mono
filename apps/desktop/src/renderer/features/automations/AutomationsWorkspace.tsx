import { useEffect, useMemo, useRef, useState } from 'react';
import type { JSX, PointerEvent as ReactPointerEvent, ReactNode } from 'react';
import { CalendarPlus, CircleAlert, Pencil, Repeat2 } from 'lucide-react';
import type {
  ApprovalRecord,
  AttemptRecord,
  AutomationSummary,
  PolicyReviewDecision,
  ProviderModelDefaults,
  ResearchModelEffortLevel,
  ResearchModelProviderId,
  ResearchModelSelection,
  ResearchProviderModelCatalog,
  RunDetail,
  SteeringAction,
  WorkspaceRegistryEntry,
  WorkspaceScopeVersion
} from '@shared/types';
import { CenteredLoadingState } from '../../app/CenteredLoadingState';
import { CollectionSidebar } from '../../app/CollectionSidebar';
import { formatSessionDateTime, traceLabel } from '../../lib/formatting';
import { buildTraceDisplayEvents } from '../../view-models/traceDisplay';
import { CommentaryView } from '../commentary/CommentaryView';
import { repeatScheduleLabel } from '../../../shared/repeatSchedule';

export function AutomationsSidebar({
  automations,
  workspaces,
  workspaceOpen,
  workspaceRegistryLoading,
  selectedWorkspaceId,
  selectedAutomation,
  collapsed,
  busy,
  loading,
  error,
  onSelectAutomation,
  onStartNewResearch,
  onStartNewResearchForWorkspace,
  onAddWorkspace,
  serverSelector,
  onResizePointerDown
}: {
  automations: readonly AutomationSummary[];
  workspaces: readonly WorkspaceRegistryEntry[];
  workspaceOpen: boolean;
  workspaceRegistryLoading: boolean;
  selectedWorkspaceId: string | null;
  selectedAutomation: AutomationSummary | null;
  collapsed: boolean;
  busy: boolean;
  loading: boolean;
  error: string | null;
  onSelectAutomation: (automation: AutomationSummary | null) => void;
  onStartNewResearch: () => void;
  onStartNewResearchForWorkspace: (workspace: WorkspaceRegistryEntry) => void;
  onAddWorkspace: () => void;
  serverSelector?: ReactNode;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
}): JSX.Element {
  const [workspacePickerOpen, setWorkspacePickerOpen] = useState(false);
  const workspacePickerRef = useRef<HTMLDivElement | null>(null);
  const scheduleButtonRef = useRef<HTMLButtonElement | null>(null);
  const firstWorkspaceOptionRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    if (workspaceOpen || collapsed) setWorkspacePickerOpen(false);
  }, [workspaceOpen, collapsed]);
  useEffect(() => {
    if (workspacePickerOpen) firstWorkspaceOptionRef.current?.focus();
  }, [workspacePickerOpen, workspaceRegistryLoading, workspaces.length]);
  useEffect(() => {
    if (!workspacePickerOpen) return undefined;
    const dismissOnOutsidePointer = (event: PointerEvent): void => {
      if (!workspacePickerRef.current?.contains(event.target as Node)) setWorkspacePickerOpen(false);
    };
    const dismissOnEscape = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setWorkspacePickerOpen(false);
      scheduleButtonRef.current?.focus();
    };
    document.addEventListener('pointerdown', dismissOnOutsidePointer);
    document.addEventListener('keydown', dismissOnEscape);
    return () => {
      document.removeEventListener('pointerdown', dismissOnOutsidePointer);
      document.removeEventListener('keydown', dismissOnEscape);
    };
  }, [workspacePickerOpen]);
  const upcoming = automations
    .filter((automation) => automation.enabled && (!selectedWorkspaceId || automation.workspaceId === selectedWorkspaceId))
    .sort((left, right) => left.title.localeCompare(right.title));
  return (
    <CollectionSidebar
      title="Automations"
      label="Automations sidebar"
      collapsed={collapsed}
      error={error}
      updateKey={`${selectedWorkspaceId ?? 'all'}:${upcoming.map((automation) => `${automation.workspaceId}:${automation.runId}`).join(',')}`}
      onResizePointerDown={onResizePointerDown}
      serverSelector={serverSelector}
      primaryAction={
        <div className="sidebar-new-research-anchor" ref={workspacePickerRef}>
          <button
            ref={scheduleButtonRef}
            type="button"
            className="sidebar-utility-button"
            aria-haspopup={!workspaceOpen ? 'menu' : undefined}
            aria-expanded={!workspaceOpen ? workspacePickerOpen : undefined}
            disabled={busy}
            onClick={() => workspaceOpen ? onStartNewResearch() : setWorkspacePickerOpen((open) => !open)}
          >
            <CalendarPlus size={15} aria-hidden="true" />
            <span>Schedule a Job</span>
          </button>
          {workspacePickerOpen && !workspaceOpen ? (
            <div className="sidebar-new-research-menu" role="menu" aria-label="Choose workspace for scheduled research">
              {workspaceRegistryLoading ? (
                <div className="sidebar-new-research-menu-empty" role="status">Loading workspaces…</div>
              ) : workspaces.length === 0 ? (
                <>
                  <div className="sidebar-new-research-menu-empty">No workspaces yet.</div>
                  <button ref={firstWorkspaceOptionRef} type="button" role="menuitem" onClick={() => {
                    setWorkspacePickerOpen(false);
                    onAddWorkspace();
                  }}>Create Workspace</button>
                </>
              ) : workspaces.map((workspace, index) => (
                <button
                  key={workspace.id}
                  ref={index === 0 ? firstWorkspaceOptionRef : undefined}
                  type="button"
                  role="menuitem"
                  title={workspace.workspacePath}
                  aria-label={`Schedule research in ${workspace.workspaceName}`}
                  onClick={() => {
                    setWorkspacePickerOpen(false);
                    onStartNewResearchForWorkspace(workspace);
                  }}
                >{workspace.workspaceName}</button>
              ))}
            </div>
          ) : null}
        </div>
      }
    >
      <nav className="collection-sidebar-list sidebar-list-scroll-content" aria-label="Automation jobs">
        <div className="collection-sidebar-list-heading">Upcoming</div>
        {loading ? <p className="collection-sidebar-empty">Loading jobs…</p> : upcoming.length === 0 ? (
          <p className="collection-sidebar-empty">No upcoming jobs</p>
        ) : upcoming.map((automation) => {
          const selected = selectedAutomation?.runId === automation.runId && selectedAutomation.workspaceId === automation.workspaceId;
          return (
            <div className={`workspace-item-row no-menu ${selected ? 'active' : ''}`.trim()} key={`${automation.workspaceId}:${automation.runId}`}>
              <button type="button" className="workspace-item collection-sidebar-item" aria-current={selected ? 'page' : undefined} onClick={() => onSelectAutomation(automation)}>
                <span className="collection-sidebar-item-copy">
                  <span>{automation.title}</span>
                  <small>{repeatScheduleLabel(automation.schedule)} · {automation.workspaceName}</small>
                </span>
              </button>
            </div>
          );
        })}
      </nav>
    </CollectionSidebar>
  );
}

export function AutomationsWorkspace({
  automations,
  workspaces,
  selectedWorkspaceId,
  selectedAutomation,
  detail,
  sessionSetupPending,
  activeScope,
  providerModelDefaults,
  providerModelCatalog,
  shellApproval,
  shellApprovalBusy,
  responseSuggestionsEnabled,
  busy,
  loading,
  error,
  onScopeChange,
  onSelectAutomation,
  onShellApprovalDecision,
  onSessionAction,
  onSteerInstruction
}: {
  automations: readonly AutomationSummary[];
  workspaces: readonly WorkspaceRegistryEntry[];
  selectedWorkspaceId: string | null;
  selectedAutomation: AutomationSummary | null;
  detail: RunDetail | null;
  sessionSetupPending: boolean;
  activeScope: WorkspaceScopeVersion | null;
  providerModelDefaults: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>> | undefined;
  providerModelCatalog: ResearchProviderModelCatalog[];
  shellApproval?: ApprovalRecord | null;
  shellApprovalBusy?: boolean;
  responseSuggestionsEnabled: boolean;
  busy: boolean;
  loading: boolean;
  error: string | null;
  onScopeChange: (workspaceId: string | null) => void;
  onSelectAutomation: (automation: AutomationSummary | null) => void;
  onShellApprovalDecision: (decision: PolicyReviewDecision) => void;
  onSessionAction: (action: SteeringAction) => void;
  onSteerInstruction: (runId: string, instruction: string, modelSelection: ResearchModelSelection) => void;
}): JSX.Element {
  const scoped = useMemo(
    () => selectedWorkspaceId ? automations.filter((automation) => automation.workspaceId === selectedWorkspaceId) : [...automations],
    [automations, selectedWorkspaceId]
  );
  const scopeTabs = [
    { id: null, key: 'all', label: 'All Automations' },
    ...workspaces
      .filter((workspace) => workspace.workspaceId.length > 0)
      .map((workspace) => ({ id: workspace.workspaceId, key: workspace.id, label: workspace.workspaceName }))
  ];
  const currentScopeName = selectedWorkspaceId
    ? workspaces.find((workspace) => workspace.workspaceId === selectedWorkspaceId)?.workspaceName ?? 'Workspace'
    : 'All';

  if (selectedAutomation) {
    return (
      <AutomationSessionWorkspace
        automation={selectedAutomation}
        detail={detail?.run.id === selectedAutomation.runId ? detail : null}
        sessionSetupPending={sessionSetupPending}
        activeScope={activeScope}
        providerModelDefaults={providerModelDefaults}
        providerModelCatalog={providerModelCatalog}
        shellApproval={shellApproval}
        shellApprovalBusy={shellApprovalBusy}
        responseSuggestionsEnabled={responseSuggestionsEnabled}
        busy={busy}
        onShellApprovalDecision={onShellApprovalDecision}
        onSessionAction={onSessionAction}
        onSteerInstruction={onSteerInstruction}
      />
    );
  }

  return (
    <section className="automations-workspace" aria-label="Automations" aria-busy={loading}>
      <div className="wide-content-container">
        <div className="automations-workspace-tabs research-side-view-tabs research-side-view-tabs-scrollable pill-view-tabs" role="tablist" aria-label="Automation workspace scope">
          {scopeTabs.map((scope) => {
            const selected = selectedWorkspaceId === scope.id;
            return (
              <div className={`research-side-view-tab provider-settings-tab automations-workspace-tab ${selected ? 'active' : ''}`.trim()} key={scope.key}>
                <button
                  type="button"
                  className="research-side-view-tab-activate"
                  role="tab"
                  aria-selected={selected}
                  aria-controls="automations-workspace-panel"
                  onClick={() => onScopeChange(scope.id)}
                >
                  <span>{scope.label}</span>
                </button>
              </div>
            );
          })}
        </div>
        <header className="resource-workspace-heading">
          <h1>{currentScopeName} Automations</h1>
          <p>Manage scheduled research sessions across your workspaces.</p>
        </header>
        <div className="automations-workspace-content" id="automations-workspace-panel" role="tabpanel">
          <div className="automations-workspace-catalog">
            {loading ? (
              <CenteredLoadingState label="Loading automations…" />
            ) : error ? (
              <AutomationEmptyState label="Automations could not be loaded" detail={error} error />
            ) : scoped.length === 0 ? (
              <AutomationEmptyState label="No automations yet" detail="Repeat schedules added from New Research will appear here." />
            ) : (
              <div className="automations-workspace-list">
                {scoped.map((automation) => (
                  <div className="automation-row" key={`${automation.workspaceId}:${automation.runId}`}>
                    <span className="automation-row-copy">
                      <strong>{automation.title}</strong>
                      <small>{automation.enabled ? 'Active' : 'Inactive'}</small>
                    </span>
                    <button
                      type="button"
                      className="automation-edit-button"
                      onClick={() => onSelectAutomation(automation)}
                    >
                      <Pencil size={14} aria-hidden="true" />
                      <span>Edit</span>
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>
    </section>
  );
}

export function AutomationSessionWorkspace({
  automation,
  detail,
  sessionSetupPending,
  activeScope,
  providerModelDefaults,
  providerModelCatalog,
  shellApproval = null,
  shellApprovalBusy = false,
  responseSuggestionsEnabled,
  busy,
  onShellApprovalDecision,
  onSessionAction,
  onSteerInstruction
}: {
  automation: AutomationSummary;
  detail: RunDetail | null;
  sessionSetupPending: boolean;
  activeScope: WorkspaceScopeVersion | null;
  providerModelDefaults: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>> | undefined;
  providerModelCatalog: ResearchProviderModelCatalog[];
  shellApproval?: ApprovalRecord | null;
  shellApprovalBusy?: boolean;
  responseSuggestionsEnabled: boolean;
  busy: boolean;
  onShellApprovalDecision: (decision: PolicyReviewDecision) => void;
  onSessionAction: (action: SteeringAction) => void;
  onSteerInstruction: (runId: string, instruction: string, modelSelection: ResearchModelSelection) => void;
}): JSX.Element {
  const attempts = useMemo(() => orderedAutomationAttempts(detail?.attempts ?? []), [detail?.attempts]);
  const [selectedAttemptId, setSelectedAttemptId] = useState<string | null>(null);

  useEffect(() => {
    setSelectedAttemptId((current) => attempts.some((attempt) => attempt.id === current)
      ? current
      : attempts[0]?.id ?? null);
  }, [attempts]);

  const selectedAttempt = attempts.find((attempt) => attempt.id === selectedAttemptId) ?? attempts[0] ?? null;
  const commentaryDetail = useMemo(
    () => automationAttemptDetail(detail, selectedAttempt?.id ?? null),
    [detail, selectedAttempt?.id]
  );
  const events = useMemo(
    () => commentaryDetail ? buildTraceDisplayEvents(commentaryDetail) : [],
    [commentaryDetail]
  );
  const initialModelSelection: ResearchModelSelection = {
    provider: automation.settings.provider as ResearchModelProviderId,
    model: automation.settings.model,
    reasoningEffort: automation.settings.reasoningEffort as ResearchModelEffortLevel
  };

  return (
    <div className="report-session-grid automation-session-grid">
      <div className="automation-session-commentary">
        <CommentaryView
          key={`${automation.workspaceId}:${automation.runId}`}
          busy={busy}
          detail={commentaryDetail}
          sessionSetupPending={sessionSetupPending}
          events={events}
          activeScope={activeScope}
          providerModelCatalog={providerModelCatalog}
          providerModelDefaults={providerModelDefaults}
          selectedRunId={automation.runId}
          showBackToMain={false}
          scrollScopeKey={`${automation.runId}:${selectedAttempt?.id ?? 'latest'}`}
          searchHighlightQuery=""
          shellApproval={shellApproval}
          shellApprovalBusy={shellApprovalBusy}
          initialModelSelection={initialModelSelection}
          collaboration={automation.settings.collaboration}
          initialSafetyMode={automation.settings.shellSafetyMode}
          initialInstruction={automation.settings.promptMarkdown}
          inputPlaceholder="Automation prompt"
          responseSuggestionsEnabled={responseSuggestionsEnabled}
          onBackToMain={() => undefined}
          onShellApprovalDecision={onShellApprovalDecision}
          onSessionAction={onSessionAction}
          onSteerInstruction={onSteerInstruction}
        />
      </div>
      <div className="report-session-sidenav-gutter" aria-hidden="true" />
      <AutomationRunsSidebar
        attempts={attempts}
        intervalLabel={repeatScheduleLabel(automation.schedule)}
        selectedAttemptId={selectedAttempt?.id ?? null}
        onSelectAttempt={setSelectedAttemptId}
      />
    </div>
  );
}

export function orderedAutomationAttempts(attempts: readonly AttemptRecord[]): AttemptRecord[] {
  return [...attempts].sort((left, right) => Date.parse(right.startedAt) - Date.parse(left.startedAt));
}

export function automationAttemptDetail(detail: RunDetail | null, attemptId: string | null): RunDetail | null {
  if (!detail || !attemptId) return detail;
  return {
    ...detail,
    traceEvents: detail.traceEvents.filter((event) => event.attemptId === attemptId),
    transcriptMessages: detail.transcriptMessages.filter((message) => message.attemptId === attemptId),
    breakoutRooms: detail.breakoutRooms?.filter((room) => room.attemptId === attemptId),
    breakoutRoomMembers: detail.breakoutRoomMembers?.filter((member) => member.attemptId === attemptId),
    breakoutRoomMessages: detail.breakoutRoomMessages?.filter((message) => message.attemptId === attemptId),
    verifierRuns: detail.verifierRuns.filter((run) => run.attemptId === attemptId),
    contextCompactions: detail.contextCompactions.filter((compaction) => compaction.attemptId === attemptId),
    policyEvents: detail.policyEvents.filter((event) => event.attemptId === attemptId)
  };
}

function AutomationRunsSidebar({
  attempts,
  intervalLabel,
  selectedAttemptId,
  onSelectAttempt
}: {
  attempts: readonly AttemptRecord[];
  intervalLabel: string;
  selectedAttemptId: string | null;
  onSelectAttempt: (attemptId: string) => void;
}): JSX.Element {
  return (
    <aside className="main-session-side session-summary-panel automation-summary-panel" aria-label="Automation">
      <section className="session-summary-card">
        <header className="session-summary-heading">
          <h2 className="session-summary-title">Automation</h2>
        </header>
        <section className="session-summary-items" aria-label="Automation details">
          <div className="session-summary-item automation-interval-row">
            <Repeat2 size={15} aria-hidden="true" />
            <span>Interval</span>
            <span className="session-summary-meta">{intervalLabel}</span>
          </div>
        </section>
        <hr className="session-summary-divider automation-run-divider" />
        <section className="automation-run-list" aria-label="Automation run history">
          {attempts.length === 0 ? (
            <p className="automation-run-list-empty">No runs recorded yet.</p>
          ) : attempts.map((attempt) => {
            const selected = attempt.id === selectedAttemptId;
            return (
              <button
                type="button"
                className={`automation-run-row ${selected ? 'selected' : ''}`.trim()}
                aria-pressed={selected}
                key={attempt.id}
                onClick={() => onSelectAttempt(attempt.id)}
              >
                <time dateTime={attempt.startedAt} title={formatSessionDateTime(attempt.startedAt)}>
                  {formatSessionDateTime(attempt.startedAt)}
                </time>
                <span>{traceLabel(attempt.status)}</span>
              </button>
            );
          })}
        </section>
      </section>
    </aside>
  );
}

function AutomationEmptyState({ label, detail, error = false }: {
  label: string;
  detail?: string;
  error?: boolean;
}): JSX.Element {
  return (
    <div className={`automations-workspace-empty ${error ? 'is-error' : ''}`.trim()} role={error ? 'alert' : 'status'}>
      {error ? <CircleAlert size={20} aria-hidden="true" /> : null}
      <strong>{label}</strong>
      {detail ? <span>{detail}</span> : null}
    </div>
  );
}
