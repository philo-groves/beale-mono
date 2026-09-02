import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, JSX, ReactNode } from 'react';
import type { ApprovalRecord, AppServerMemorySummary, AppServerReportDocument, AppServerReportSummary, AppServerRunbookDocument, AppServerRunbookSummary, MemoryDreamingProgressUpdate, PolicyReviewDecision, ProviderModelDefaults, RepositoryCloneMode, ResearchKitId, ResearchKitRefreshInput, ResearchKitRefreshResult, ResearchModelProviderId, ResearchModelSelection, ResearchProfile, ResearchProviderModelCatalog, RunDetail, RunRow, RunbookExecutionSelection, RunbookProofTarget, RunbookProofTargetSelection, ScopeAssetInput, SteeringAction, TraceEventRecord, WorkspaceDejunkSummary, WorkspaceMemoryBackendId, WorkspaceRule, WorkspaceScopeVersion } from '@shared/types';
import { WorkspaceUnderstandingView } from '../workspaces/WorkspaceUnderstandingView';
import type { WorkspaceConfigurationInput, WorkspaceDashboardView } from '../workspaces/WorkspaceUnderstandingView';
import { ResearchSidePanel } from '../research/MemorySidePanel';
import { CommentaryView } from '../commentary/CommentaryView';
import { ConnectedDeviceCapture } from '../deviceCapture/ConnectedDeviceCapture';
import { isEndedResearchRunStatus, SessionNextSteps, type ResearchGoalSeed } from './SessionNextSteps';
import { EMPTY_SESSION_HEAT_PREFERENCES } from '../../view-models/sessionHeat';
import type { SessionHeatPreferences } from '../../view-models/sessionHeat';
import type { TraceDisplayEvent } from '../../view-models/traceDisplay';
import {
  MIN_TRACE_PANEL_WIDTH,
  RESEARCH_SIDE_RESIZE_HANDLE_WIDTH,
  MIN_RESEARCH_SIDE_PANEL_WIDTH,
  useResizableResearchSidePanel
} from '../../hooks/useResizableResearchSidePanel';

const WORKSPACE_DETAIL_TRANSITION_MS = 220;
const CONNECTED_DEVICE_DEFAULT_ASPECT_RATIO = 1290 / 2796;
const CONNECTED_DEVICE_CAPTURE_HORIZONTAL_INSET = 18;
const CONNECTED_DEVICE_CAPTURE_VERTICAL_INSET = 22;
const RESEARCH_SIDE_PANEL_UPDATE_MS = 2_000;

export type MainSessionViewState = 'new-research' | 'session' | 'workspace';

export function mainSessionViewState(newResearchOpen: boolean, selectedRunId: string | null): MainSessionViewState {
  if (newResearchOpen) return 'new-research';
  return selectedRunId ? 'session' : 'workspace';
}

export const MainSessionWorkspace = memo(function MainSessionWorkspace({
  detail,
  sessionSetupPending = false,
  events,
  allEvents,
  providerModelCatalog,
  providerModelDefaults,
  appServerMemory,
  activeScope = null,
  workspaceRules = [],
  researchProfile,
  researchKitId = 'general',
  sessionHeatPreferences = EMPTY_SESSION_HEAT_PREFERENCES,
  sessionEndingSuggestionsEnabled = true,
  responseSuggestionsEnabled = true,
  researchSubjectName = '',
  workspacePath = '',
  workspaceDirectories,
  workspaceMemoryBackend = 'app-server',
  workspaceName,
  viewState: viewStateInput,
  newResearchContent = null,
  initialWorkspaceView = 'campaign',
  runs,
  selectedRunId,
  researchDetailsOpen,
  selectedRunbookId,
  selectedRunbook,
  selectedRunbookDocument,
  runbookLoading,
  runbookError,
  selectedReportId = null,
  selectedReport = null,
  selectedReportDocument = null,
  reportLoading = false,
  reportError = null,
  selectedSubagentPath,
  searchHighlightQuery,
  shellApproval = null,
  shellApprovalBusy = false,
  dangerModeEnabled = false,
  busy,
  connectedDeviceCaptureEnabled = false,
  workspaceDejunk = null,
  workspaceDejunkInProgress = false,
  memoryDreamingInProgress,
  memoryDreamingProgress = null,
  onRunWorkspaceDejunk = () => undefined,
  onRunMemoryDreaming,
  onAddWorkspaceResource = async () => undefined,
  onChangeWorkspaceResource = async () => undefined,
  onCloneWorkspaceRepository = async () => undefined,
  onRefreshWorkspaceResearchKit = async () => { throw new Error('Research Kit refresh is unavailable.'); },
  onAddWorkspaceRule = async () => undefined,
  onSaveWorkspaceConfiguration = async () => undefined,
  onChangeWorkspaceDirectories = async () => undefined,
  onChangeWorkspaceMemoryBackend = async () => undefined,
  onRemoveWorkspace = async () => undefined,
  onOpenSession = () => undefined,
  onWorkspaceViewChange,
  onResearchDetailsOpenChange,
  onOpenAppServerRunbook,
  onRunAppServerRunbook = async () => undefined,
  onBackToRunbooks,
  onOpenAppServerReport = () => undefined,
  onBackToReports = () => undefined,
  onBackToSubagents,
  onSelectSubagent,
  onSelectNextStep,
  onShellApprovalDecision = () => undefined,
  onSessionAction,
  onSteerInstruction
}: {
  detail: RunDetail | null;
  sessionSetupPending?: boolean;
  events: TraceDisplayEvent[];
  allEvents: TraceDisplayEvent[];
  providerModelCatalog: ResearchProviderModelCatalog[];
  providerModelDefaults?: Partial<Record<ResearchModelProviderId, ProviderModelDefaults>>;
  appServerMemory: AppServerMemorySummary | null;
  activeScope?: WorkspaceScopeVersion | null;
  workspaceRules?: WorkspaceRule[];
  researchProfile: ResearchProfile | null;
  researchKitId?: ResearchKitId;
  sessionHeatPreferences?: SessionHeatPreferences;
  sessionEndingSuggestionsEnabled?: boolean;
  responseSuggestionsEnabled?: boolean;
  researchSubjectName?: string;
  workspacePath?: string;
  workspaceDirectories?: readonly string[];
  workspaceMemoryBackend?: WorkspaceMemoryBackendId;
  workspaceName: string;
  viewState?: MainSessionViewState;
  newResearchContent?: ReactNode;
  initialWorkspaceView?: WorkspaceDashboardView;
  runs: RunRow[];
  selectedRunId: string | null;
  researchDetailsOpen: boolean;
  selectedRunbookId: string | null;
  selectedRunbook: AppServerRunbookSummary | null;
  selectedRunbookDocument: AppServerRunbookDocument | null;
  runbookLoading: boolean;
  runbookError: string | null;
  selectedReportId?: string | null;
  selectedReport?: AppServerReportSummary | null;
  selectedReportDocument?: AppServerReportDocument | null;
  reportLoading?: boolean;
  reportError?: string | null;
  selectedSubagentPath: string | null;
  searchHighlightQuery: string;
  shellApproval?: ApprovalRecord | null;
  shellApprovalBusy?: boolean;
  dangerModeEnabled?: boolean;
  busy: boolean;
  connectedDeviceCaptureEnabled?: boolean;
  workspaceDejunk?: WorkspaceDejunkSummary | null;
  workspaceDejunkInProgress?: boolean;
  memoryDreamingInProgress: boolean;
  memoryDreamingProgress?: MemoryDreamingProgressUpdate | null;
  onRunWorkspaceDejunk?: () => void;
  onRunMemoryDreaming: () => void;
  onAddWorkspaceResource?: (asset: ScopeAssetInput) => Promise<void>;
  onChangeWorkspaceResource?: (assetIds: string[], asset: ScopeAssetInput | null) => Promise<void>;
  onCloneWorkspaceRepository?: (assetId: string, cloneMode: RepositoryCloneMode) => Promise<void>;
  onRefreshWorkspaceResearchKit?: (input: ResearchKitRefreshInput) => Promise<ResearchKitRefreshResult>;
  onAddWorkspaceRule?: (text: string) => Promise<void>;
  onSaveWorkspaceConfiguration?: (configuration: WorkspaceConfigurationInput) => Promise<void>;
  onChangeWorkspaceDirectories?: (directories: string[]) => Promise<void>;
  onChangeWorkspaceMemoryBackend?: (memoryBackend: WorkspaceMemoryBackendId) => Promise<void>;
  onRemoveWorkspace?: () => Promise<void>;
  onOpenSession?: (runId: string) => void;
  onWorkspaceViewChange?: (viewName: string) => void;
  onResearchDetailsOpenChange: (expanded: boolean) => void;
  onOpenAppServerRunbook: (runbookId: string) => void;
  onRunAppServerRunbook?: (runbookId: string, selection: RunbookExecutionSelection, target: RunbookProofTargetSelection) => Promise<void>;
  onBackToRunbooks: () => void;
  onOpenAppServerReport?: (reportId: string) => void;
  onBackToReports?: () => void;
  onBackToSubagents: () => void;
  onSelectSubagent: (path: string) => void;
  onSelectNextStep: (goal: ResearchGoalSeed) => void;
  onShellApprovalDecision?: (decision: PolicyReviewDecision) => void;
  onSessionAction: (action: SteeringAction) => void;
  onSteerInstruction: (runId: string, instruction: string, modelSelection: ResearchModelSelection) => void;
}): JSX.Element | null {
  const [selectedWorkspaceClaimId, setSelectedWorkspaceClaimId] = useState<string | null>(null);
  const [selectedWorkspaceMemoryId, setSelectedWorkspaceMemoryId] = useState<string | null>(null);
  const [connectedDeviceCaptureVisible, setConnectedDeviceCaptureVisible] = useState(false);
  const [connectedDeviceCaptureExpanded, setConnectedDeviceCaptureExpanded] = useState(false);
  const [connectedDeviceOs, setConnectedDeviceOs] = useState<string | null>(null);
  const [connectedDeviceAspectRatio, setConnectedDeviceAspectRatio] = useState(CONNECTED_DEVICE_DEFAULT_ASPECT_RATIO);
  const [mainSessionSize, setMainSessionSize] = useState({ width: 0, height: 0 });
  const autoExpandedRunbookRunIdRef = useRef<string | null>(null);
  const viewState = viewStateInput ?? mainSessionViewState(false, selectedRunId);
  const workspaceView = viewState === 'workspace';
  const [workspaceSidePanelMounted, setWorkspaceSidePanelMounted] = useState(workspaceView && researchDetailsOpen);
  const [workspaceSidePanelVisible, setWorkspaceSidePanelVisible] = useState(workspaceView && researchDetailsOpen);
  const sessionHasContent = viewState === 'session' && selectedRunId !== null && sessionContentAvailable(detail, events);
  const showResearchSidePanel = sessionHasContent || workspaceSidePanelMounted;
  const visibleResearchDetails = selectedRunId !== null ? sessionHasContent && researchDetailsOpen : workspaceSidePanelVisible;
  const expandedResearchSidePanel = selectedRunId !== null ? sessionHasContent && researchDetailsOpen : workspaceSidePanelMounted;
  const researchSideResizeEnabled = sessionHasContent && !visibleResearchDetails && !connectedDeviceCaptureExpanded;
  const {
    containerRef,
    panelWidth,
    maximumPanelWidth,
    beginResize,
    handleResizeKeyDown
  } = useResizableResearchSidePanel(showResearchSidePanel);
  useEffect(() => {
    const container = containerRef.current;
    if (!container || typeof ResizeObserver === 'undefined') return undefined;
    const updateSize = (): void => {
      const bounds = container.getBoundingClientRect();
      setMainSessionSize((current) => current.width === bounds.width && current.height === bounds.height
        ? current
        : { width: bounds.width, height: bounds.height });
    };
    updateSize();
    const observer = new ResizeObserver(updateSize);
    observer.observe(container);
    return () => observer.disconnect();
  }, [containerRef]);
  const viewSpace = viewState === 'session' ? 'session' : 'workspace';
  const researchSidePanelKey = selectedRunId ?? `workspace:${appServerMemory?.contextWorkspaceId ?? 'current'}`;
  const researchSideData = useCoalescedResearchSideData({
    detail,
    events: allEvents,
    key: `${researchSidePanelKey}:${selectedSubagentPath ?? 'root'}`,
    coalesce: viewSpace === 'session' && detail?.run.status === 'active'
  });
  useEffect(() => {
    setSelectedWorkspaceClaimId(null);
    setSelectedWorkspaceMemoryId(null);
  }, [appServerMemory?.contextWorkspaceId, selectedRunId]);
  useEffect(() => {
    if (!workspaceView) {
      setWorkspaceSidePanelMounted(false);
      setWorkspaceSidePanelVisible(false);
      return;
    }
    let animationFrame: number | null = null;
    let closeTimer: number | null = null;
    if (researchDetailsOpen) {
      setWorkspaceSidePanelMounted(true);
      animationFrame = window.requestAnimationFrame(() => setWorkspaceSidePanelVisible(true));
    } else {
      setWorkspaceSidePanelVisible(false);
      closeTimer = window.setTimeout(() => {
        setWorkspaceSidePanelMounted(false);
        setSelectedWorkspaceClaimId(null);
        setSelectedWorkspaceMemoryId(null);
      }, WORKSPACE_DETAIL_TRANSITION_MS);
    }
    return () => {
      if (animationFrame !== null) window.cancelAnimationFrame(animationFrame);
      if (closeTimer !== null) window.clearTimeout(closeTimer);
    };
  }, [researchDetailsOpen, workspaceView]);

  const openWorkspaceClaim = (claimId: string): void => {
    onBackToRunbooks();
    setSelectedWorkspaceMemoryId(null);
    setSelectedWorkspaceClaimId(claimId);
    onResearchDetailsOpenChange(true);
  };
  const openWorkspaceMemory = (nodeId: string): void => {
    onBackToRunbooks();
    setSelectedWorkspaceClaimId(null);
    setSelectedWorkspaceMemoryId(nodeId);
    onResearchDetailsOpenChange(true);
  };
  const openWorkspaceRunbook = (runbookId: string): void => {
    setSelectedWorkspaceClaimId(null);
    setSelectedWorkspaceMemoryId(null);
    onOpenAppServerRunbook(runbookId);
    onResearchDetailsOpenChange(true);
  };
  const closeWorkspaceClaim = (): void => {
    onResearchDetailsOpenChange(false);
  };
  const closeWorkspaceMemory = (): void => {
    onResearchDetailsOpenChange(false);
  };
  const closeWorkspaceRunbook = (): void => {
    onBackToRunbooks();
    onResearchDetailsOpenChange(false);
  };
  const changeResearchDetailsOpen = useCallback((expanded: boolean): void => {
    onResearchDetailsOpenChange(expanded);
  }, [onResearchDetailsOpenChange]);
  useEffect(() => {
    if (researchDetailsOpen) {
      autoExpandedRunbookRunIdRef.current = null;
      setConnectedDeviceCaptureExpanded(false);
    }
  }, [researchDetailsOpen]);
  useEffect(() => {
    if (!connectedDeviceCaptureVisible) {
      autoExpandedRunbookRunIdRef.current = null;
      setConnectedDeviceCaptureExpanded(false);
    }
  }, [connectedDeviceCaptureVisible]);
  const latestRunbookExecution = latestOverallRunbookExecution(detail?.traceEvents ?? []);
  useEffect(() => {
    if (!latestRunbookExecution) {
      if (autoExpandedRunbookRunIdRef.current) {
        autoExpandedRunbookRunIdRef.current = null;
        setConnectedDeviceCaptureExpanded(false);
      }
      return;
    }
    const running = latestRunbookExecution.status === 'queued' || latestRunbookExecution.status === 'running';
    if (
      running
      && !connectedDeviceCaptureExpanded
      && !researchDetailsOpen
      && connectedDeviceCaptureVisible
      && latestRunbookExecution.proofTarget === 'device'
      && isIosDeviceOs(latestRunbookExecution.deviceOs)
    ) {
      autoExpandedRunbookRunIdRef.current = latestRunbookExecution.runId;
      setConnectedDeviceCaptureExpanded(true);
      return;
    }
    if (!running && autoExpandedRunbookRunIdRef.current === latestRunbookExecution.runId) {
      autoExpandedRunbookRunIdRef.current = null;
      setConnectedDeviceCaptureExpanded(false);
    }
  }, [connectedDeviceCaptureVisible, latestRunbookExecution?.deviceOs, latestRunbookExecution?.proofTarget, latestRunbookExecution?.runId, latestRunbookExecution?.status, researchDetailsOpen]);

  const changeConnectedDeviceCaptureExpanded = (expanded: boolean): void => {
    autoExpandedRunbookRunIdRef.current = null;
    setConnectedDeviceCaptureExpanded(expanded);
  };
  const expandedDeviceCaptureWidth = useMemo(() => connectedDeviceCaptureExpanded
    ? expandedDeviceCapturePanelWidth(mainSessionSize.width, mainSessionSize.height, connectedDeviceAspectRatio)
    : null, [connectedDeviceAspectRatio, connectedDeviceCaptureExpanded, mainSessionSize.height, mainSessionSize.width]);

  const postSessionContent = detail && shouldShowSessionNextSteps(detail.run.status, sessionEndingSuggestionsEnabled)
    ? (
        <SessionNextSteps
          key={detail.run.id}
          detail={detail}
          onSelect={onSelectNextStep}
        />
      )
    : null;

  return (
    <div
      ref={containerRef}
      className={`main-session-grid${workspaceView ? ' workspace-context' : ''}${visibleResearchDetails || (viewState === 'session' && connectedDeviceCaptureExpanded) ? ' research-details-open' : ''}${workspaceView && !visibleResearchDetails ? ' workspace-main-only' : ''}${viewState === 'new-research' || (selectedRunId !== null && !sessionHasContent) ? ' session-main-only' : ''}`}
      data-session-view-state={viewState}
      style={{
        '--research-side-panel-width': `${panelWidth}px`,
        ...(expandedDeviceCaptureWidth !== null && expandedDeviceCaptureWidth > 0
          ? { '--research-side-panel-active-width': `${expandedDeviceCaptureWidth}px` }
          : {})
      } as CSSProperties}
    >
      {viewState === 'new-research' ? newResearchContent : workspaceView ? (
        <WorkspaceUnderstandingView
          key={workspacePath}
          busy={busy}
          activeScope={activeScope}
          workspaceRules={workspaceRules}
          workspaceDejunk={workspaceDejunk}
          workspaceDejunkInProgress={workspaceDejunkInProgress}
          memoryDreamingInProgress={memoryDreamingInProgress}
          memoryDreamingProgress={memoryDreamingProgress}
          appServerMemory={appServerMemory}
          researchProfile={researchProfile}
          researchKitId={researchKitId}
          sessionHeatPreferences={sessionHeatPreferences}
          researchSubjectName={researchSubjectName}
          workspacePath={workspacePath}
          workspaceDirectories={workspaceDirectories}
          memoryBackend={workspaceMemoryBackend}
          providerModelCatalog={providerModelCatalog}
          selectedClaimId={selectedWorkspaceClaimId}
          workspaceName={workspaceName}
          initialView={initialWorkspaceView}
          runs={runs}
          onRunWorkspaceDejunk={onRunWorkspaceDejunk}
          onRunMemoryDreaming={onRunMemoryDreaming}
          onAddResource={onAddWorkspaceResource}
          onChangeResource={onChangeWorkspaceResource}
          onCloneRepository={onCloneWorkspaceRepository}
          onRefreshResearchKit={onRefreshWorkspaceResearchKit}
          onAddRule={onAddWorkspaceRule}
          onSaveConfiguration={onSaveWorkspaceConfiguration}
          onChangeWorkspaceDirectories={onChangeWorkspaceDirectories}
          onChangeMemoryBackend={onChangeWorkspaceMemoryBackend}
          onRemoveWorkspace={onRemoveWorkspace}
          onOpenSession={onOpenSession}
          onActiveViewChange={onWorkspaceViewChange}
          onOpenClaim={openWorkspaceClaim}
          onOpenMemory={openWorkspaceMemory}
          onOpenRunbook={openWorkspaceRunbook}
        />
      ) : (
        <CommentaryView
          busy={busy}
          dangerModeEnabled={dangerModeEnabled}
          detail={detail}
          sessionSetupPending={sessionSetupPending}
          events={events}
          activeScope={activeScope}
          providerModelCatalog={providerModelCatalog}
          providerModelDefaults={providerModelDefaults}
          selectedRunId={selectedRunId}
          showBackToMain={false}
          searchHighlightQuery={searchHighlightQuery}
          shellApproval={shellApproval}
          shellApprovalBusy={shellApprovalBusy}
          postSessionContent={postSessionContent}
          responseSuggestionsEnabled={responseSuggestionsEnabled}
          onBackToMain={() => undefined}
          onShellApprovalDecision={onShellApprovalDecision}
          onSessionAction={onSessionAction}
          onSteerInstruction={onSteerInstruction}
        />
      )}
      {showResearchSidePanel ? (
        <div
          className="research-side-resize-handle"
          role="separator"
          aria-label={selectedRunId ? 'Resize Runbooks, Reports, Subagents, and Memories sidebar' : 'Resize workspace detail sidebar'}
          aria-orientation="vertical"
          aria-valuemin={MIN_RESEARCH_SIDE_PANEL_WIDTH}
          aria-valuemax={maximumPanelWidth}
          aria-valuenow={panelWidth}
          aria-hidden={!researchSideResizeEnabled}
          tabIndex={researchSideResizeEnabled ? 0 : -1}
          onKeyDown={researchSideResizeEnabled ? handleResizeKeyDown : undefined}
          onPointerDown={researchSideResizeEnabled ? beginResize : undefined}
        />
      ) : null}
      {showResearchSidePanel ? <div className={`research-side-column${connectedDeviceCaptureVisible ? ' has-connected-device-capture' : ''}${connectedDeviceCaptureExpanded ? ' device-capture-expanded' : ''}`}>
        <ResearchSidePanel
          detail={researchSideData.detail}
          events={researchSideData.events}
          memory={viewSpace === 'workspace' ? appServerMemory : researchSideData.detail?.appServerMemory ?? null}
          researchProfile={researchProfile}
          sessionHeatPreferences={sessionHeatPreferences}
          providerModelCatalog={providerModelCatalog}
          runId={researchSidePanelKey}
          runStatus={researchSideData.detail?.run.status ?? null}
          expanded={expandedResearchSidePanel}
          selectedRunbook={selectedRunbook}
          selectedRunbookDocument={selectedRunbookDocument}
          selectedRunbookId={selectedRunbookId}
          selectedClaimId={!selectedRunId ? selectedWorkspaceClaimId : undefined}
          selectedMemoryNodeId={!selectedRunId ? selectedWorkspaceMemoryId : undefined}
          runbookLoading={runbookLoading}
          runbookError={runbookError}
          connectedDeviceOs={connectedDeviceOs}
          selectedReport={selectedReport}
          selectedReportDocument={selectedReportDocument}
          selectedReportId={selectedReportId}
          reportLoading={reportLoading}
          reportError={reportError}
          selectedSubagentPath={selectedSubagentPath}
          searchHighlightQuery={searchHighlightQuery}
          onSelectSubagent={onSelectSubagent}
          onOpenClaim={!selectedRunId ? openWorkspaceClaim : undefined}
          onOpenRunbook={selectedRunId ? onOpenAppServerRunbook : openWorkspaceRunbook}
          onRunbookExecute={selectedRunId ? onRunAppServerRunbook : undefined}
          onBackToRunbooks={selectedRunId ? onBackToRunbooks : closeWorkspaceRunbook}
          onBackToClaim={!selectedRunId ? closeWorkspaceClaim : undefined}
          onBackToMemory={!selectedRunId ? closeWorkspaceMemory : undefined}
          onOpenReport={onOpenAppServerReport}
          onBackToReports={onBackToReports}
          onBackToSubagents={onBackToSubagents}
          onExpandedChange={changeResearchDetailsOpen}
          viewSpace={viewSpace}
        />
        {viewState === 'session' && selectedRunId ? (
          <ConnectedDeviceCapture
            active={connectedDeviceCaptureEnabled}
            expanded={connectedDeviceCaptureExpanded}
            onAspectRatioChange={setConnectedDeviceAspectRatio}
            onDeviceOsChange={setConnectedDeviceOs}
            onExpandedChange={changeConnectedDeviceCaptureExpanded}
            onVisibilityChange={setConnectedDeviceCaptureVisible}
          />
        ) : null}
      </div> : null}
    </div>
  );
});

interface ResearchSideData {
  detail: RunDetail | null;
  events: TraceDisplayEvent[];
}

function useCoalescedResearchSideData({
  detail,
  events,
  key,
  coalesce
}: ResearchSideData & { key: string; coalesce: boolean }): ResearchSideData {
  const [current, setCurrent] = useState<ResearchSideData>({ detail, events });
  const currentKeyRef = useRef(key);
  const pendingRef = useRef<ResearchSideData>({ detail, events });
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    pendingRef.current = { detail, events };
    if (!coalesce || currentKeyRef.current !== key) {
      currentKeyRef.current = key;
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      setCurrent(pendingRef.current);
      return;
    }
    if (timerRef.current !== null) return;
    timerRef.current = window.setTimeout(() => {
      timerRef.current = null;
      setCurrent(pendingRef.current);
    }, RESEARCH_SIDE_PANEL_UPDATE_MS);
  }, [coalesce, detail, events, key]);

  useEffect(() => () => {
    if (timerRef.current !== null) window.clearTimeout(timerRef.current);
  }, []);

  if (currentKeyRef.current !== key) return { detail, events };
  return current;
}

export interface RunbookExecutionLifecycle {
  runId: string;
  status: string;
  proofTarget: RunbookProofTarget;
  deviceOs: string | null;
}

export function expandedDeviceCapturePanelWidth(
  containerWidth: number,
  containerHeight: number,
  aspectRatio: number
): number {
  if (containerWidth <= 0 || containerHeight <= 0 || !Number.isFinite(aspectRatio) || aspectRatio <= 0) return 0;
  const desiredWidth = Math.max(0, containerHeight - CONNECTED_DEVICE_CAPTURE_VERTICAL_INSET) * aspectRatio
    + CONNECTED_DEVICE_CAPTURE_HORIZONTAL_INSET;
  const maximumWidth = Math.max(0, containerWidth - MIN_TRACE_PANEL_WIDTH - RESEARCH_SIDE_RESIZE_HANDLE_WIDTH);
  return Math.round(Math.min(desiredWidth, maximumWidth));
}

export function latestOverallRunbookExecution(events: readonly TraceEventRecord[]): RunbookExecutionLifecycle | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const payload = events[index]?.payload;
    if (!payload || payload.eventType !== 'runbook_execution' || payload.cellId !== null) continue;
    const runId = typeof payload.runbookRunId === 'string' ? payload.runbookRunId : '';
    const status = typeof payload.status === 'string' ? payload.status : '';
    const proofTarget = payload.proofTarget;
    if (!runId || !status || !isRunbookProofTarget(proofTarget)) continue;
    return {
      runId,
      status,
      proofTarget,
      deviceOs: typeof payload.deviceOs === 'string' ? payload.deviceOs : null
    };
  }
  return null;
}

export function isIosDeviceOs(deviceOs: string | null): boolean {
  return Boolean(deviceOs && /^(?:ios|iphone os)(?:\s|\d|$)/i.test(deviceOs.trim()));
}

export function sessionContentAvailable(
  detail: RunDetail | null,
  events: readonly TraceDisplayEvent[]
): boolean {
  return Boolean(detail && (detail.run.promptMarkdown.trim() || events.length > 0));
}

function isRunbookProofTarget(value: unknown): value is RunbookProofTarget {
  return value === 'localhost' || value === 'device' || value === 'vm' || value === 'web' || value === 'other';
}

export function shouldShowSessionNextSteps(
  status: RunDetail['run']['status'] | null,
  enabled = true
): boolean {
  return enabled && status !== null && isEndedResearchRunStatus(status);
}
