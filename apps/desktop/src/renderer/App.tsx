import { useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { JSX } from 'react';
import type { CSSProperties } from 'react';
import 'katex/dist/katex.min.css';
import type { BealeWorkspaceResearchTrack, FleetAppServer, FleetAppServerStatus, FleetRemoteCatalog, FleetState } from '@beale/app-server-runtime/protocol';
import './styles.css';
import { devInstrumentation, useDevInputLatencyProbe, useDevRenderProbe } from './devInstrumentation';
import type {
  ApprovalRecord,
  AgentPluginRegistryState,
  AppServerRemoteAccessSettings,
  AppServerRemoteAccessUpdate,
  AutomationSummary,
  ProviderSettings,
  ProviderAuthenticationMethod,
  ProviderContextSize,
  ProviderModelDefaults,
  AppServerRunbookDocument,
  AppServerReportDocument,
  AppServerReportSummary,
  AppServerFindingSummary,
  ClaimBoardMaturity,
  NotificationRecord,
  OpenAiOAuthStartResult,
  PolicyReviewDecision,
  RepositoryCloneMode,
  ResearchModelSelection,
  ResearchModelProviderId,
  ResearchProviderId,
  ResearchProviderOAuthStartResult,
  ResearchProviderModelCatalog,
  ResearchKitRefreshInput,
  ResearchKitRefreshResult,
  ResolvedResearchProfile,
  ResearchProviderStatus,
  ResearchSessionSummary,
  StartRunInput,
  RunDetailProjection,
  RunRecord,
  RunStatus,
  RunbookExecutionSelection,
  ScopeAssetInput,
  WorkspaceOnboardingProgressUpdate,
  RunDetail,
  ShellSafetyMode,
  SteeringAction,
  TicketingMode,
  TicketingProviderId,
  TicketingSettings,
  TicketingTarget,
  WorkspaceEditorCatalog,
  WorkspaceEditorId,
  WorkspaceMemoryBackendId,
  WorkspaceRegistryEntry,
  WorkspaceSnapshot
} from '@shared/types';
import { AppModals } from './app/AppModals';
import { AppNavigationRail, resolveAppNavigationDestination } from './app/AppNavigationRail';
import { TopBar } from './app/TopBar';
import { SidebarServerSelector } from './app/SidebarServerSelector';
import { NotificationStack, type WorkspaceAlert } from './features/notifications/Notifications';
import { WorkspaceSidebar } from './features/workspaces/WorkspaceSidebar';
import { RemoteFleetSidebar, RemoteFleetWorkspace } from './features/fleet/RemoteFleetWorkspace';
import { RemoteQuickChatDock, type RemoteQuickChatDescriptor } from './features/fleet/RemoteQuickChatDock';
import { remoteRunStatus, remoteWorkspaceRegistry } from './features/fleet/remoteFleetProjection';
import { FleetSidebar, type FleetSelection } from './features/fleet/FleetSidebar';
import { FleetSettingsView } from './features/settings/FleetSettingsView';
import { QuickChatDock, type QuickChatDescriptor } from './features/quick-chat/QuickChatDock';
import { WorkspaceStartupView } from './features/workspaces/WorkspaceStartupView';
import { WorkspaceCreationView } from './features/workspaces/WorkspaceCreationView';
import { MainSessionWorkspace } from './features/sessions/MainSessionWorkspace';
import { StartRunForm } from './features/sessions/StartRunForm';
import { workspaceScopeDraftForConfigurationUpdate } from './features/workspaces/WorkspaceUnderstandingView';
import type { WorkspaceConfigurationInput, WorkspaceDashboardView } from './features/workspaces/WorkspaceUnderstandingView';
import { ReportSessionWorkspace } from './features/reports/ReportsWorkspace';
import { AutomationsSidebar, AutomationsWorkspace } from './features/automations/AutomationsWorkspace';
import { PluginManagerWorkspace, PluginsSidebar } from './features/plugins/PluginManagerWorkspace';
import {
  isInlineApproval,
  pendingShellApproval,
  ShellApprovalModal
} from './features/sessions/ShellApprovalModal';
import { subagentSummaries } from './view-models/subagents';
import { surfaceFocusSummary, type SurfaceFocusSummary } from './view-models/surfaceFocus';
import { SettingsSidebar, SettingsView, settingsSectionHeaderIcon, settingsSectionLabel, type SettingsSection } from './features/settings/SettingsModal';
import { useInsetScrollbarActivation } from './hooks/useInsetScrollbarActivation';
import { useWorkspaceActions, type WorkspaceActionOptions } from './hooks/useWorkspaceActions';
import { useProfilingRuntime } from './hooks/useProfilingRuntime';
import { useResizableSidebar } from './hooks/useResizableSidebar';
import { useRunDetailPolling } from './hooks/useRunDetailPolling';
import { useResearchTrackSessionRunning } from './hooks/useResearchTrackSessionRunning';
import { useResearchGoalSuggestions } from './hooks/useResearchGoalSuggestions';
import { useSidebarPerformanceProbe } from './hooks/useSidebarPerformanceProbe';
import { usePermissionSettings } from './hooks/usePermissionSettings';
import { permissionAllowsSteeringAction } from './view-models/permissionSettings';
import { useAppearanceTheme } from './hooks/useAppearanceTheme';
import { useSessionHeatPreferences } from './hooks/useSessionHeatPreferences';
import { useSuggestionPreferences } from './hooks/useSuggestionPreferences';
import { filterEnabledProviderModelCatalogs } from '../shared/optionalProviderModels';
import { useWorkspaceRuntime } from './hooks/useWorkspaceRuntime';
import { errorMessage, isWorkspacePrimaryDirectoryMissingError } from './lib/errors';
import {
  activeRunDetailForSelection,
  appShellClassName,
  selectedRunStatus,
  shouldShowHeaderResearchControls,
  windowControlPlatformForState
} from './view-models/appShell';
import type { WorkspaceOnboardingFormState } from './view-models/workspaceOnboarding';
import {
  EMPTY_SESSION_HEAT_DISPLAY_STATE,
  sessionHeatDisplayStateForSelection,
  sessionHeatPaletteForProfile,
  sessionHeatPaletteStyle
} from './view-models/sessionHeat';
import { buildTraceDisplayEvents, buildTraceDisplayEventsForAgentPath } from './view-models/traceDisplay';
import { runDetailMetricDetail, shortMetricId } from './view-models/runDetailUpdates';
import { deferredResearchTrackDetail, pauseResearchTrackSessionAction, runningResearchTrackDetail, waitForResearchTrackSession } from './view-models/researchTrackSession';
import { hasResearchProfileDetailFeatures, researchProfileFeatureAvailability } from './view-models/researchProfileFeatures';
import { isReportResourceRun, reportSessionDefaultModelSelection, reportTitleFromMarkdown } from './view-models/reports';
import {
  clearConfirmedProviderOAuthResults,
  isSubscriptionAuthenticationConfirmed
} from './view-models/providerAuthentication';

export function App(): JSX.Element {
  const appShellRef = useRef<HTMLDivElement | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [workspaceEditors, setWorkspaceEditors] = useState<WorkspaceEditorCatalog | null>(null);
  const [quickChats, setQuickChats] = useState<QuickChatDescriptor[]>([]);
  const openQuickChat = useCallback((): void => {
    setQuickChats((current) => [...current, { id: `quick-chat-${crypto.randomUUID()}` }]);
  }, []);
  const closeQuickChat = useCallback((id: string): void => {
    setQuickChats((current) => current.filter((chat) => chat.id !== id));
  }, []);
  const registerQuickChatRun = useCallback((id: string, run: RunRecord): void => {
    setQuickChats((current) => current.map((chat) => (
      chat.id === id ? { ...chat, runId: run.id, runState: run.status } : chat
    )));
  }, []);
  const resumeQuickChat = useCallback(async (session: ResearchSessionSummary): Promise<void> => {
    setQuickChats((current) => current.some((chat) => chat.runId === session.runId)
      ? current
      : [...current, {
          id: `quick-chat-${crypto.randomUUID()}`,
          runId: session.runId,
          runState: session.status
        }]);
  }, []);
  const handleError = useCallback((message: string) => setError(message), []);
  const {
    snapshot,
    workspaceRegistry,
    hostEnvironment,
    windowChromeState,
    openAiStatus,
    startupPhase,
    selectedRunId,
    setWorkspaceRegistry,
    setOpenAiStatus,
    setSelectedRunId,
    selectPendingRunId,
    releasePendingRunId,
    applySnapshot,
    loadSnapshot,
    loadWorkspaceRegistry
  } = useWorkspaceRuntime(handleError);
  const [fleetServers, setFleetServers] = useState<FleetAppServer[]>([]);
  const [fleetServerStatuses, setFleetServerStatuses] = useState<Record<string, FleetAppServerStatus>>({});
  const fleetStatusRequestId = useRef(0);
  const [selectedFleetServerId, setSelectedFleetServerId] = useState('local');
  const refreshFleetServerStatuses = useCallback((): void => {
    const requestId = ++fleetStatusRequestId.current;
    void window.beale.getFleetAppServerStatuses().then((statuses) => {
      if (fleetStatusRequestId.current !== requestId) return;
      setFleetServerStatuses(Object.fromEntries(statuses.map((status) => [status.id, status])));
    }).catch(() => {
      if (fleetStatusRequestId.current !== requestId) return;
      setFleetServerStatuses({ local: { id: 'local', state: 'offline', message: 'Could not check the local app server.' } });
    });
  }, []);
  const [remoteQuickChats, setRemoteQuickChats] = useState<RemoteQuickChatDescriptor[]>([]);
  const [remoteCatalog, setRemoteCatalog] = useState<FleetRemoteCatalog | null>(null);
  const [remoteCatalogError, setRemoteCatalogError] = useState<string | null>(null);
  const [remoteWorkspaceId, setRemoteWorkspaceId] = useState<string | null>(null);
  const [remoteSessionId, setRemoteSessionId] = useState<string | null>(null);
  const [remoteWorkspaceViewName, setRemoteWorkspaceViewName] = useState('Campaign Findings');
  useEffect(() => { setRemoteWorkspaceViewName('Campaign Findings'); }, [selectedFleetServerId, remoteWorkspaceId]);
  const [remoteSurfaceFocus, setRemoteSurfaceFocus] = useState<{
    serverId: string; workspaceId: string; sessionId: string; summary: SurfaceFocusSummary;
  } | null>(null);
  const [remoteSelectedSubagentPath, setRemoteSelectedSubagentPath] = useState<string | null>(null);
  const onRemoteSurfaceFocusChange = useCallback((serverId: string, workspaceId: string, sessionId: string, summary: SurfaceFocusSummary | null): void => {
    setRemoteSurfaceFocus(summary ? { serverId, workspaceId, sessionId, summary } : null);
  }, []);
  const [remoteResearchOpen, setRemoteResearchOpen] = useState(false);
  const [remoteTrackMode, setRemoteTrackMode] = useState(false);
  useEffect(() => {
    setRemoteSurfaceFocus(null);
    setRemoteSelectedSubagentPath(null);
  }, [selectedFleetServerId, remoteWorkspaceId, remoteSessionId]);
  useEffect(() => {
    if (startupPhase !== 'ready') return;
    refreshFleetServerStatuses();
    const interval = window.setInterval(refreshFleetServerStatuses, 20_000);
    return () => { window.clearInterval(interval); fleetStatusRequestId.current += 1; };
  }, [refreshFleetServerStatuses, startupPhase]);
  useEffect(() => {
    if (startupPhase !== 'ready') return;
    let active = true;
    const refresh = (): void => { void window.beale.getFleetState().then((state) => {
      if (active) setFleetServers(state.enabled && state.role === 'primary' ? state.appServers : []);
    }).catch(() => undefined); };
    refresh();
    const interval = window.setInterval(refresh, 20_000);
    return () => { active = false; window.clearInterval(interval); };
  }, [startupPhase]);
  useEffect(() => {
    if (selectedFleetServerId !== 'local' && !fleetServers.some((server) => server.id === selectedFleetServerId)) {
      setSelectedFleetServerId('local');
    }
  }, [fleetServers, selectedFleetServerId]);
  useEffect(() => {
    if (selectedFleetServerId === 'local') { setRemoteCatalog(null); setRemoteCatalogError(null); return; }
    let active = true;
    const refresh = (): void => { void window.beale.getFleetRemoteCatalog(selectedFleetServerId).then((catalog) => {
      if (active) { setRemoteCatalog(catalog); setRemoteCatalogError(null); }
    }).catch((caught: unknown) => { if (active) setRemoteCatalogError(errorMessage(caught)); }); };
    refresh();
    const interval = window.setInterval(refresh, 5_000);
    return () => { active = false; window.clearInterval(interval); };
  }, [selectedFleetServerId]);
  useEffect(() => {
    if (startupPhase !== 'ready') return;
    let cancelled = false;
    window.beale.getWorkspaceEditors()
      .then((catalog) => {
        if (!cancelled) setWorkspaceEditors(catalog);
      })
      .catch((caught: unknown) => {
        if (!cancelled) handleError(errorMessage(caught));
      });
    return () => {
      cancelled = true;
    };
  }, [handleError, startupPhase]);

  useEffect(() => {
    if (startupPhase !== 'ready') return;
    let cancelled = false;
    window.beale.getAppServerRemoteAccessSettings(true)
      .then((settings) => {
        if (!cancelled) setAppServerRemoteAccessSettings(settings);
      })
      .catch((caught: unknown) => {
        if (!cancelled) handleError(errorMessage(caught));
      });
    return () => {
      cancelled = true;
    };
  }, [handleError, startupPhase]);

  useEffect(() => {
    if (startupPhase !== 'ready') return;
    let cancelled = false;
    window.beale.getDebuggingSettings()
      .then((settings) => {
        if (!cancelled) setTracesEnabled(settings.tracesEnabled);
      })
      .catch((caught: unknown) => {
        if (!cancelled) handleError(errorMessage(caught));
      });
    return () => {
      cancelled = true;
    };
  }, [handleError, startupPhase]);

  const openWorkspaceInEditor = useCallback((editorId: WorkspaceEditorId): void => {
    setError(null);
    void window.beale.openWorkspaceInEditor(editorId).catch((caught: unknown) => setError(errorMessage(caught)));
  }, []);
  const changeTracesEnabled = useCallback((enabled: boolean): void => {
    setTracesEnabled(enabled);
    void window.beale.setTracesEnabled(enabled)
      .then((settings) => setTracesEnabled(settings.tracesEnabled))
      .catch((caught: unknown) => handleError(errorMessage(caught)));
  }, [handleError]);
  const detectAppServerRemoteAccess = useCallback(async (): Promise<void> => {
    setAppServerRemoteAccessBusy(true);
    try {
      setAppServerRemoteAccessSettings(await window.beale.getAppServerRemoteAccessSettings(true));
    } catch (caught: unknown) {
      handleError(errorMessage(caught));
    } finally {
      setAppServerRemoteAccessBusy(false);
    }
  }, [handleError]);
  const changeAppServerRemoteAccess = useCallback(async (
    update: AppServerRemoteAccessUpdate
  ): Promise<void> => {
    setAppServerRemoteAccessBusy(true);
    try {
      setAppServerRemoteAccessSettings(await window.beale.setAppServerRemoteAccessSettings(update));
    } catch (caught: unknown) {
      handleError(errorMessage(caught));
    } finally {
      setAppServerRemoteAccessBusy(false);
    }
  }, [handleError]);
  const pendingViewedSessionIdsRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!selectedRunId || !workspaceRegistry) return;
    const selectedSession = workspaceRegistry.researchSessions.find((session) => session.runId === selectedRunId);
    if (!selectedSession || selectedSession.resultViewedAt !== null || !isEndedResearchRunStatus(selectedSession.status)) return;
    if (pendingViewedSessionIdsRef.current.has(selectedSession.id)) return;
    pendingViewedSessionIdsRef.current.add(selectedSession.id);
    void window.beale.markResearchSessionViewed(selectedSession.id)
      .then(setWorkspaceRegistry)
      .catch((caught: unknown) => setError(errorMessage(caught)))
      .finally(() => pendingViewedSessionIdsRef.current.delete(selectedSession.id));
  }, [selectedRunId, setWorkspaceRegistry, workspaceRegistry]);
  const [suggestionPreferences, setSuggestionPreference] = useSuggestionPreferences();
  const researchGoalSuggestionState = useResearchGoalSuggestions(
    snapshot,
    openAiStatus?.configured ?? snapshot?.openAi.configured ?? false,
    suggestionPreferences.newResearchPromptSuggestionsEnabled
  );
  const [openAiOAuthResult, setOpenAiOAuthResult] = useState<OpenAiOAuthStartResult | null>(null);
  const [researchProviderStatuses, setResearchProviderStatuses] = useState<ResearchProviderStatus[]>([]);
  const [researchProviderStatusesLoaded, setResearchProviderStatusesLoaded] = useState(false);
  const [researchProviderModelCatalog, setResearchProviderModelCatalog] = useState<ResearchProviderModelCatalog[]>([]);
  const [researchProviderOAuthResults, setResearchProviderOAuthResults] = useState<Partial<Record<ResearchProviderId, ResearchProviderOAuthStartResult>>>({});
  const [providerSettings, setProviderSettings] = useState<ProviderSettings | null>(null);
  const [researchProfiles, setResearchProfiles] = useState<ResolvedResearchProfile[]>([]);
  const [researchProfilesLoading, setResearchProfilesLoading] = useState(false);
  const enabledResearchProviderModelCatalog = useMemo(
    () => filterEnabledProviderModelCatalogs(researchProviderModelCatalog, providerSettings),
    [providerSettings, researchProviderModelCatalog]
  );
  const quickChatInitialModelSelection = useMemo(
    () => {
      const leadProvider = providerSettings?.defaultProviderId;
      if (!providerSettings || !leadProvider) return null;
      const preferredModel = providerSettings.modelDefaults[leadProvider]?.largeModel
        ?? (leadProvider === 'openai-codex'
          ? openAiStatus?.defaultModel
          : researchProviderStatuses.find((status) => status.id === leadProvider)?.defaultModel);
      if (!preferredModel) return null;
      return reportSessionDefaultModelSelection(
        providerSettings,
        enabledResearchProviderModelCatalog,
        openAiStatus,
        researchProviderStatuses
      );
    },
    [enabledResearchProviderModelCatalog, openAiStatus, providerSettings, researchProviderStatuses]
  );
  const [tracesEnabled, setTracesEnabled] = useState(false);
  const [appServerRemoteAccessSettings, setAppServerRemoteAccessSettings] = useState<AppServerRemoteAccessSettings | null>(null);
  const [appServerRemoteAccessBusy, setAppServerRemoteAccessBusy] = useState(false);
  const [appearanceTheme, setAppearanceTheme] = useAppearanceTheme();
  const [permissionSettings, setDangerModeEnabled, setDefaultShellSafetyMode, setGuardianEnabled] = usePermissionSettings();
  const [sessionHeatPreferences, setSessionHeatPreference, setSessionHeatPalettePreference] = useSessionHeatPreferences();
  const [workspaceDraft, setWorkspaceDraft] = useState<WorkspaceOnboardingFormState | null>(null);
  const [workspaceOnboardingProgress, setWorkspaceOnboardingProgress] = useState<WorkspaceOnboardingProgressUpdate | null>(null);
  const [missingDirectoryWorkspace, setMissingDirectoryWorkspace] = useState<WorkspaceRegistryEntry | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [fleetOpen, setFleetOpen] = useState(false);
  const [fleetConfiguration, setFleetConfiguration] = useState<FleetState | null>(null);
  const [fleetConfigurationError, setFleetConfigurationError] = useState<string | null>(null);
  const [fleetSelection, setFleetSelection] = useState<FleetSelection>({ section: 'app-servers', itemId: null });
  const [fleetScopeWorkspaceId, setFleetScopeWorkspaceId] = useState<string | null>(null);
  const applyFleetConfiguration = useCallback((next: FleetState): void => {
    setFleetConfiguration(next);
    setFleetServers(next.enabled && next.role === 'primary' ? next.appServers : []);
    refreshFleetServerStatuses();
  }, [refreshFleetServerStatuses]);
  const refreshFleetConfiguration = useCallback(async (): Promise<void> => {
    const next = await window.beale.getFleetState();
    applyFleetConfiguration(next);
    setFleetConfigurationError(null);
  }, [applyFleetConfiguration]);
  useEffect(() => {
    if (!fleetOpen) return;
    void refreshFleetConfiguration().catch((caught: unknown) => setFleetConfigurationError(errorMessage(caught)));
  }, [fleetOpen, refreshFleetConfiguration]);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('general');
  const [workspaceDashboardViewName, setWorkspaceDashboardViewName] = useState('Campaign Findings');
  const [workspaceDashboardInitialView, setWorkspaceDashboardInitialView] = useState<WorkspaceDashboardView>('board');
  const workspaceDashboardWorkspaceIdRef = useRef<string | null>(null);
  useEffect(() => {
    const workspaceId = snapshot?.workspace.workspaceId ?? null;
    if (workspaceDashboardWorkspaceIdRef.current === workspaceId) return;
    workspaceDashboardWorkspaceIdRef.current = workspaceId;
    setWorkspaceDashboardInitialView('board');
    if (workspaceId) setWorkspaceDashboardViewName('Campaign Findings');
  }, [snapshot?.workspace.workspaceId]);
  const [newResearchOpen, setNewResearchOpen] = useState(false);
  const [newResearchTrackMode, setNewResearchTrackMode] = useState(false);
  const [pendingTrackNavigation, setPendingTrackNavigation] = useState<{
    track: BealeWorkspaceResearchTrack;
    serverId: string | null;
  } | null>(null);
  const navigateToResearchTrack = useCallback((track: BealeWorkspaceResearchTrack, serverId: string | null): void => {
    if (serverId) setRemoteSessionId(track.id);
    else selectPendingRunId(track.id, track.workspaceId);
    setPendingTrackNavigation({ track, serverId });
  }, [selectPendingRunId]);
  const [openScheduleOnNewResearch, setOpenScheduleOnNewResearch] = useState(false);
  const [newResearchInitialInput, setNewResearchInitialInput] = useState<StartRunInput | null>(null);
  const closeNewResearch = useCallback((): void => {
    setNewResearchOpen(false);
    setNewResearchTrackMode(false);
    setOpenScheduleOnNewResearch(false);
    setNewResearchInitialInput(null);
  }, []);
  const [pluginsOpen, setPluginsOpen] = useState(false);
  const [selectedPluginId, setSelectedPluginId] = useState<string | null>(null);
  const [pluginScopeWorkspaceId, setPluginScopeWorkspaceId] = useState<string | null>(null);
  const [automationsOpen, setAutomationsOpen] = useState(false);
  const [automationScopeWorkspaceId, setAutomationScopeWorkspaceId] = useState<string | null>(null);
  const [automations, setAutomations] = useState<AutomationSummary[]>([]);
  const [automationsLoading, setAutomationsLoading] = useState(false);
  const [automationsError, setAutomationsError] = useState<string | null>(null);
  const [selectedAutomationRunId, setSelectedAutomationRunId] = useState<string | null>(null);
  const [selectedAutomationWorkspaceId, setSelectedAutomationWorkspaceId] = useState<string | null>(null);
  const [reportsOpen, setReportsOpen] = useState(false);
  const [selectedReportOverride, setSelectedReportOverride] = useState<AppServerReportSummary | null>(null);
  const [reportSessionRunId, setReportSessionRunId] = useState<string | null>(null);
  const [reportSessionRefreshVersion, setReportSessionRefreshVersion] = useState(0);
  const [agentPluginState, setAgentPluginState] = useState<AgentPluginRegistryState | null>(null);
  const [ticketingSettings, setTicketingSettings] = useState<TicketingSettings | null>(null);
  const [ticketingTargets, setTicketingTargets] = useState<TicketingTarget[]>([]);
  const [ticketingLoading, setTicketingLoading] = useState(false);
  const [ticketingError, setTicketingError] = useState<string | null>(null);
  const [agentPluginsLoading, setAgentPluginsLoading] = useState(false);
  const [agentPluginsBusy, setAgentPluginsBusy] = useState(false);
  const [agentPluginsError, setAgentPluginsError] = useState<string | null>(null);
  const [pluginRepositoryUrl, setPluginRepositoryUrl] = useState('');
  const [profilingOpen, setProfilingOpen] = useState(false);
  const [activeNotification, setActiveNotification] = useState<NotificationRecord | null>(null);
  const [workspaceAlerts, setWorkspaceAlerts] = useState<WorkspaceAlert[]>([]);
  const [selectedSubagentPath, setSelectedSubagentPath] = useState<string | null>(null);
  const [archivedQuickChats, setArchivedQuickChats] = useState<ResearchSessionSummary[]>([]);
  const [archiveLoading, setArchiveLoading] = useState(false);
  const closedQuickChats = useMemo(() => {
    const openRunIds = new Set(quickChats.flatMap((chat) => chat.runId ? [chat.runId] : []));
    return archivedQuickChats.filter((session) => !openRunIds.has(session.runId));
  }, [archivedQuickChats, quickChats]);
  const [selectedRunbookId, setSelectedRunbookId] = useState<string | null>(null);
  const [runbookOpenVersion, setRunbookOpenVersion] = useState(0);
  const [selectedReportId, setSelectedReportId] = useState<string | null>(null);
  const [selectedReportWorkspaceId, setSelectedReportWorkspaceId] = useState<string | null>(null);
  const [rightSidenavExpanded, setRightSidenavExpanded] = useState(false);
  const [selectedRunbookDocument, setSelectedRunbookDocument] = useState<AppServerRunbookDocument | null>(null);
  const [runbookLoading, setRunbookLoading] = useState(false);
  const [runbookError, setRunbookError] = useState<string | null>(null);
  const runbookExecutionRequestRef = useRef(0);
  const runbookDocumentFetchRef = useRef<{
    runbookId: string;
    request: Promise<AppServerRunbookDocument>;
  } | null>(null);
  const [selectedReportDocument, setSelectedReportDocument] = useState<AppServerReportDocument | null>(null);
  const [reportSubmissionPacketPath, setReportSubmissionPacketPath] = useState<{
    workspaceId: string;
    reportId: string;
    artifactId: string;
    path: string;
  } | null>(null);
  const [reportLoading, setReportLoading] = useState(false);
  const [reportError, setReportError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [continuationPendingRunId, setContinuationPendingRunId] = useState<string | null>(null);
  const [shellApprovalDecisionInFlight, setShellApprovalDecisionInFlight] = useState<string | null>(null);
  const shellApprovalDecisionRef = useRef<string | null>(null);
  const { sidebarWidth, sidebarCollapsed, sidebarToggleProfile, toggleSidebar, beginSidebarResize } = useResizableSidebar();
  const {
    profilingState,
    lastProfilingReport,
    setProfilingEnabled,
    flushProfilingReport
  } = useProfilingRuntime(handleError, {
    active: startupPhase === 'ready' || profilingOpen || settingsOpen,
    observeReports: profilingOpen || settingsOpen
  });
  const selectedResearchTrackRunning = useResearchTrackSessionRunning(snapshot?.workspace.workspaceId, selectedRunId);
  const selectedRunState = selectedRunStatus(snapshot, selectedRunId, workspaceRegistry?.researchSessions);
  const runDetailProjection = useMemo<RunDetailProjection>(() => ({
    mode: 'commentary',
    agentPath: selectedSubagentPath
  }), [selectedSubagentPath]);
  const selectedRunRefreshKey = useMemo(() => {
    const selected = snapshot?.runs.find((row) => row.run.id === selectedRunId)?.run;
    if (!selected) return null;
    const pendingApprovalIds = snapshot?.pendingShellApprovals
      .filter((approval) => approval.runId === selected.id)
      .map((approval) => approval.id)
      .join(',') ?? '';
    return `${selected.status}:${selected.shellSafetyMode}:${pendingApprovalIds}:${reportsOpen ? reportSessionRefreshVersion : 0}`;
  }, [reportSessionRefreshVersion, reportsOpen, selectedRunId, snapshot?.pendingShellApprovals, snapshot?.runs]);
  const handleRunDetailError = useCallback((message: string) => setError(message), []);
  const trackSessionAwaitingCreation = pendingTrackNavigation?.serverId === null
    && selectedRunId === pendingTrackNavigation.track.id
    && !snapshot?.runs.some(({ run }) => run.id === selectedRunId);
  const { runDetail, sessionSetupPending, clearRunDetail, primeRunDetail, loadOlderHistory } = useRunDetailPolling({
    selectedRunId,
    selectedRunState,
    researchTrackRunning: selectedResearchTrackRunning,
    sessionAwaitingCreation: trackSessionAwaitingCreation,
    continuationPending: continuationPendingRunId === selectedRunId,
    projection: runDetailProjection,
    refreshKey: selectedRunRefreshKey,
    onError: handleRunDetailError
  });
  useEffect(() => {
    if (!pendingTrackNavigation) return undefined;
    const { track, serverId } = pendingTrackNavigation;
    const remote = serverId !== null;
    if (remote
      ? selectedFleetServerId !== serverId || remoteWorkspaceId !== track.workspaceId || remoteSessionId !== track.id
      : snapshot?.workspace.workspaceId !== track.workspaceId || selectedRunId !== track.id) {
      if (!remote) releasePendingRunId(track.id);
      setPendingTrackNavigation(null);
      return undefined;
    }
    const controller = new AbortController();
    void waitForResearchTrackSession({
      trackId: track.id,
      initialSessionId: track.activeSessionId,
      signal: controller.signal,
      getTrack: () => window.beale.getResearchTrack(track.workspaceId, serverId ?? undefined),
      findSession: async (sessionId) => {
        if (serverId) {
          const catalog = await window.beale.getFleetRemoteCatalog(serverId);
          return catalog.sessions.some((session) => session.id === sessionId && session.workspaceId === track.workspaceId)
            ? { kind: 'remote' as const, catalog, sessionId }
            : null;
        }
        const next = await window.beale.getSnapshot();
        const run = next?.workspace.workspaceId === track.workspaceId
          ? next.runs.find((row) => row.run.id === sessionId)?.run : null;
        return run && next ? { kind: 'local' as const, snapshot: next, run } : null;
      }
    }).then((session) => {
      if (controller.signal.aborted) return;
      if (session?.kind === 'remote') {
        setRemoteCatalog(session.catalog);
        setRemoteSessionId(session.sessionId);
      } else if (session?.kind === 'local') {
        primeRunDetail(session.run);
        applySnapshot(session.snapshot, session.run.id);
      }
      if (!remote) releasePendingRunId(track.id);
      if (!session) {
        if (remote) setRemoteSessionId((current) => current === track.id ? null : current);
        else setSelectedRunId((current) => current === track.id ? null : current);
      }
      setPendingTrackNavigation(null);
    }).catch((caught: unknown) => {
      if (controller.signal.aborted) return;
      setError(errorMessage(caught));
      if (!remote) releasePendingRunId(track.id);
      if (remote) setRemoteSessionId((current) => current === track.id ? null : current);
      else setSelectedRunId((current) => current === track.id ? null : current);
      setPendingTrackNavigation(null);
    });
    return () => controller.abort();
  }, [applySnapshot, pendingTrackNavigation, primeRunDetail, releasePendingRunId,
    remoteSessionId, remoteWorkspaceId, selectedFleetServerId, selectedRunId, setSelectedRunId, snapshot?.workspace.workspaceId]);
  useEffect(() => {
    if (continuationPendingRunId && (selectedRunId !== continuationPendingRunId || runDetail?.run.id === continuationPendingRunId && runDetail.run.status === 'active')) {
      setContinuationPendingRunId(null);
    }
  }, [continuationPendingRunId, runDetail?.run.id, runDetail?.run.status, selectedRunId]);
  useDevRenderProbe('app.shell', () => ({
    selectedRun: selectedRunId ? shortMetricId(selectedRunId) : 'none',
    workspaces: workspaceRegistry?.workspaces.length ?? 0,
    sessions: workspaceRegistry?.researchSessions.length ?? 0,
    traceEvents: runDetail?.traceEvents.length ?? 0,
    transcripts: runDetail?.transcriptMessages.length ?? 0
  }));
  useDevInputLatencyProbe();
  useSidebarPerformanceProbe({ appShellRef, profile: sidebarToggleProfile });
  useInsetScrollbarActivation();

  const researchViewContextKey = selectedRunId
    ?? snapshot?.workspace.workspaceId
    ?? snapshot?.workspace.workspacePath
    ?? null;
  useEffect(() => {
    setRightSidenavExpanded(false);
    setSelectedSubagentPath(null);
    setSelectedRunbookId(null);
    setSelectedRunbookDocument(null);
    setRunbookLoading(false);
    setRunbookError(null);
  }, [researchViewContextKey]);

  useEffect(() => {
    if (!newResearchOpen && !automationsOpen && !reportsOpen && quickChats.length === 0 && !(settingsOpen && settingsSection === 'providers')) return;
    window.beale
      .getProviderSettings()
      .then(setProviderSettings)
      .catch((caught: unknown) => handleError(errorMessage(caught)));
  }, [automationsOpen, handleError, newResearchOpen, quickChats.length, reportsOpen, settingsOpen, settingsSection]);

  useEffect(() => {
    if (!settingsOpen || settingsSection !== 'profile') return;
    let cancelled = false;
    setResearchProfiles([]);
    setResearchProfilesLoading(true);
    window.beale
      .getResearchProfiles()
      .then((profiles) => {
        if (!cancelled) setResearchProfiles(profiles);
      })
      .catch((caught: unknown) => {
        if (!cancelled) handleError(errorMessage(caught));
      })
      .finally(() => {
        if (!cancelled) setResearchProfilesLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [handleError, settingsOpen, settingsSection, snapshot?.workspace.workspacePath]);

  const runAction = useCallback(
    async (action: () => Promise<WorkspaceSnapshot | null | void>, options: { refreshSnapshot?: boolean } = {}) => {
      setBusy(true);
      setError(null);
      try {
        const next = await action();
        if (next) applySnapshot(next);
        else if (options.refreshSnapshot !== false) await loadSnapshot();
      } catch (caught) {
        setError(errorMessage(caught));
      } finally {
        setBusy(false);
      }
    },
    [applySnapshot, loadSnapshot]
  );

  const changeWorkspaceResource = useCallback(async (
    replacedAssetIds: string[],
    asset: ScopeAssetInput | null
  ): Promise<void> => {
    const activeScope = snapshot?.activeScope;
    if (!activeScope) throw new Error('The active workspace scope is unavailable.');
    setBusy(true);
    setError(null);
    try {
      const replacedAssetIdSet = new Set(replacedAssetIds);
      const assets: ScopeAssetInput[] = activeScope.assets
        .filter((existingAsset) => !replacedAssetIdSet.has(existingAsset.id))
        .map((existingAsset) => ({
          direction: existingAsset.direction,
          kind: existingAsset.kind,
          value: existingAsset.value,
          sensitivity: existingAsset.sensitivity,
          attributes: existingAsset.attributes
        }));
      applySnapshot(await window.beale.saveScope({
        workspaceName: activeScope.workspaceName,
        scopeOwner: activeScope.scopeOwner,
        descriptionMarkdown: activeScope.descriptionMarkdown,
        rulesMarkdown: '',
        expiresAt: activeScope.expiresAt,
        assets: asset ? [...assets, asset] : assets
      }));
    } catch (caught) {
      setError(errorMessage(caught));
      throw caught;
    } finally {
      setBusy(false);
    }
  }, [applySnapshot, snapshot?.activeScope]);

  const addWorkspaceResource = useCallback(
    (asset: ScopeAssetInput): Promise<void> => changeWorkspaceResource([], asset),
    [changeWorkspaceResource]
  );

  const cloneWorkspaceRepository = useCallback(async (assetId: string, cloneMode: RepositoryCloneMode): Promise<void> => {
    setError(null);
    try {
      applySnapshot(await window.beale.cloneWorkspaceRepository(assetId, cloneMode));
    } catch (caught) {
      setError(errorMessage(caught));
      throw caught;
    }
  }, [applySnapshot]);

  const addWorkspaceRule = useCallback(async (text: string): Promise<void> => {
    setError(null);
    try {
      applySnapshot(await window.beale.addWorkspaceRule(text));
    } catch (caught) {
      setError(errorMessage(caught));
      throw caught;
    }
  }, [applySnapshot]);

  const refreshWorkspaceResearchKit = useCallback(async (input: ResearchKitRefreshInput): Promise<ResearchKitRefreshResult> => {
    setError(null);
    try {
      const result = await window.beale.refreshResearchKit(input);
      applySnapshot(result.snapshot);
      return result;
    } catch (caught) {
      setError(errorMessage(caught));
      throw caught;
    }
  }, [applySnapshot]);

  const saveWorkspaceConfiguration = useCallback(async (
    configuration: WorkspaceConfigurationInput
  ): Promise<void> => {
    const activeScope = snapshot?.activeScope;
    if (!activeScope) throw new Error('The active workspace scope is unavailable.');
    setError(null);
    try {
      applySnapshot(await window.beale.saveScope(
        workspaceScopeDraftForConfigurationUpdate(activeScope, configuration)
      ));
    } catch (caught) {
      setError(errorMessage(caught));
      throw caught;
    }
  }, [applySnapshot, snapshot?.activeScope]);

  const changeWorkspaceDirectories = useCallback(async (directories: string[]): Promise<void> => {
    setError(null);
    try {
      applySnapshot(await window.beale.updateWorkspaceDirectories(directories));
    } catch (caught) {
      const message = errorMessage(caught);
      setError(message);
      throw caught;
    }
  }, [applySnapshot]);

  const changeWorkspaceMemoryBackend = useCallback(async (memoryBackend: WorkspaceMemoryBackendId): Promise<void> => {
    setError(null);
    try {
      applySnapshot(await window.beale.updateWorkspaceMemoryBackend(memoryBackend));
    } catch (caught) {
      const message = errorMessage(caught);
      setError(message);
      throw caught;
    }
  }, [applySnapshot]);

  const changeWorkspaceResearchSubject = useCallback(async (researchSubjectName: string): Promise<void> => {
    setError(null);
    try {
      applySnapshot(await window.beale.updateWorkspaceResearchSubject(researchSubjectName));
    } catch (caught) {
      const message = errorMessage(caught);
      setError(message);
      throw caught;
    }
  }, [applySnapshot]);

  const loadAgentPlugins = useCallback(async (): Promise<void> => {
    setAgentPluginsLoading(true);
    setAgentPluginsError(null);
    try {
      setAgentPluginState(await window.beale.getAgentPlugins());
    } catch (caught) {
      setAgentPluginsError(errorMessage(caught));
    } finally {
      setAgentPluginsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!settingsOpen) return;
    if (settingsSection === 'optional-features') void loadAgentPlugins();
  }, [loadAgentPlugins, settingsOpen, settingsSection]);

  const loadTicketingTargets = useCallback(async (providerId: TicketingProviderId): Promise<void> => {
    setTicketingLoading(true);
    setTicketingError(null);
    try {
      setTicketingTargets(await window.beale.listTicketingTargets(providerId));
    } catch (caught) {
      setTicketingTargets([]);
      setTicketingError(errorMessage(caught));
    } finally {
      setTicketingLoading(false);
    }
  }, []);

  const loadTicketingSettings = useCallback(async (loadTargets: boolean): Promise<void> => {
    setTicketingLoading(true);
    setTicketingError(null);
    try {
      const next = await window.beale.getTicketingSettings();
      setTicketingSettings(next);
      if (loadTargets && next.provider !== 'local' && next[next.provider].credentialConfigured) {
        setTicketingTargets(await window.beale.listTicketingTargets(next.provider));
      } else {
        setTicketingTargets([]);
      }
    } catch (caught) {
      setTicketingError(errorMessage(caught));
    } finally {
      setTicketingLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!settingsOpen || settingsSection !== 'ticketing') return;
    void loadTicketingSettings(true);
  }, [loadTicketingSettings, settingsOpen, settingsSection]);

  const changeTicketingProvider = useCallback(async (providerId: TicketingMode): Promise<void> => {
    setTicketingLoading(true);
    setTicketingError(null);
    try {
      const next = await window.beale.setTicketingProvider(providerId);
      setTicketingSettings(next);
      if (providerId !== 'local' && next[providerId].credentialConfigured) {
        setTicketingTargets(await window.beale.listTicketingTargets(providerId));
      } else {
        setTicketingTargets([]);
      }
    } catch (caught) {
      setTicketingError(errorMessage(caught));
    } finally {
      setTicketingLoading(false);
    }
  }, []);

  const changeTicketingHumanInTheLoop = useCallback(async (enabled: boolean): Promise<void> => {
    setTicketingLoading(true);
    setTicketingError(null);
    try {
      setTicketingSettings(await window.beale.setTicketingHumanInTheLoop(enabled));
    } catch (caught) {
      setTicketingError(errorMessage(caught));
    } finally {
      setTicketingLoading(false);
    }
  }, []);

  const configureTicketingCredential = useCallback(async (providerId: TicketingProviderId, apiKey: string): Promise<void> => {
    setTicketingLoading(true);
    setTicketingError(null);
    try {
      setTicketingSettings(await window.beale.configureTicketingCredential(providerId, apiKey));
      setTicketingTargets(await window.beale.listTicketingTargets(providerId));
    } catch (caught) {
      setTicketingError(errorMessage(caught));
      throw caught;
    } finally {
      setTicketingLoading(false);
    }
  }, []);

  const removeTicketingCredential = useCallback(async (providerId: TicketingProviderId): Promise<void> => {
    setTicketingLoading(true);
    setTicketingError(null);
    try {
      setTicketingSettings(await window.beale.removeTicketingCredential(providerId));
      setTicketingTargets([]);
    } catch (caught) {
      setTicketingError(errorMessage(caught));
    } finally {
      setTicketingLoading(false);
    }
  }, []);

  const changeTicketingTarget = useCallback(async (providerId: TicketingProviderId, target: TicketingTarget): Promise<void> => {
    setTicketingLoading(true);
    setTicketingError(null);
    try {
      setTicketingSettings(await window.beale.setTicketingTarget(providerId, target));
    } catch (caught) {
      setTicketingError(errorMessage(caught));
    } finally {
      setTicketingLoading(false);
    }
  }, []);

  const closeWorkspaceOnboarding = useCallback((): void => {
    setWorkspaceDraft(null);
    setWorkspaceOnboardingProgress(null);
    setError(null);
  }, []);

  const loadArchiveCatalog = useCallback(async (): Promise<void> => {
    setArchiveLoading(true);
    try {
      setArchivedQuickChats(await window.beale.listArchivedQuickChats());
    } catch (caught: unknown) {
      setError(errorMessage(caught));
    } finally {
      setArchiveLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!settingsOpen || settingsSection !== 'archive') return;
    void loadArchiveCatalog();
  }, [loadArchiveCatalog, settingsOpen, settingsSection]);

  const archiveResearchSession = useCallback(async (session: ResearchSessionSummary): Promise<void> => {
    try {
      setWorkspaceRegistry(await window.beale.archiveResearchSession(session.id));
      if (selectedRunId === session.runId) {
        clearRunDetail();
        setSelectedRunId(null);
      }
    } catch (caught: unknown) {
      setError(errorMessage(caught));
    }
  }, [clearRunDetail, selectedRunId, setSelectedRunId, setWorkspaceRegistry]);

  const restoreResearchSession = useCallback(async (session: ResearchSessionSummary): Promise<void> => {
    setArchiveLoading(true);
    try {
      setWorkspaceRegistry(await window.beale.restoreResearchSession(session.id));
    } catch (caught: unknown) {
      setError(errorMessage(caught));
    } finally {
      setArchiveLoading(false);
    }
  }, [setWorkspaceRegistry]);

  const openPlugins = useCallback((): void => {
    closeWorkspaceOnboarding();
    closeNewResearch();
    setSettingsOpen(false);
    setFleetOpen(false);
    clearRunDetail();
    setSelectedRunId(null);
    setReportsOpen(false);
    setAutomationsOpen(false);
    setPluginsOpen(true);
    setSelectedPluginId(null);
    void loadAgentPlugins();
  }, [clearRunDetail, closeNewResearch, closeWorkspaceOnboarding, loadAgentPlugins, setSelectedRunId]);

  const runAgentPluginAction = useCallback(async (action: () => Promise<AgentPluginRegistryState>): Promise<void> => {
    setAgentPluginsBusy(true);
    setAgentPluginsError(null);
    try {
      setAgentPluginState(await action());
    } catch (caught) {
      setAgentPluginsError(errorMessage(caught));
    } finally {
      setAgentPluginsBusy(false);
    }
  }, []);

  const addAgentPluginFromFilesystem = useCallback((): void => {
    void runAgentPluginAction(() => window.beale.addAgentPluginFromFilesystem());
  }, [runAgentPluginAction]);

  const addAgentPluginFromRepository = useCallback((): void => {
    const repositoryUrl = pluginRepositoryUrl.trim();
    if (!repositoryUrl) return;
    void runAgentPluginAction(async () => {
      const state = await window.beale.addAgentPluginFromRepository(repositoryUrl);
      setPluginRepositoryUrl('');
      return state;
    });
  }, [pluginRepositoryUrl, runAgentPluginAction]);

  const setAgentPluginEnabled = useCallback((pluginId: string, enabled: boolean): void => {
    void runAgentPluginAction(() => window.beale.setAgentPluginEnabled(pluginId, enabled));
  }, [runAgentPluginAction]);

  const setVisibleAgentPluginEnabled = useCallback((pluginId: string, enabled: boolean, workspaceId: string | null): void => {
    void runAgentPluginAction(() => window.beale.setAgentPluginEnabled(pluginId, enabled, workspaceId ?? undefined));
  }, [runAgentPluginAction]);

  const installAgentPlugin = useCallback((pluginId: string): void => {
    setSelectedPluginId(pluginId);
    void runAgentPluginAction(() => window.beale.setAgentPluginEnabled(pluginId, true));
  }, [runAgentPluginAction]);

  const removeAgentPlugin = useCallback((pluginId: string): void => {
    void runAgentPluginAction(async () => {
      const state = await window.beale.removeAgentPlugin(pluginId);
      setSelectedPluginId((current) => current === pluginId ? null : current);
      return state;
    });
  }, [runAgentPluginAction]);

  const openNotification = useCallback(
    async (notification: NotificationRecord) => {
      setActiveNotification(notification);
      try {
        applySnapshot(await window.beale.openNotification(notification.id));
      } catch (caught) {
        setError(errorMessage(caught));
      }
    },
    [applySnapshot]
  );

  const dismissNotification = useCallback(
    async (notificationId: string) => {
      try {
        applySnapshot(await window.beale.dismissNotification(notificationId));
      } catch (caught) {
        setError(errorMessage(caught));
      }
    },
    [applySnapshot]
  );

  const dismissWorkspaceAlert = useCallback((alertId: string) => {
    setWorkspaceAlerts((current) => current.filter((alert) => alert.id !== alertId));
  }, []);

  const openWorkspaceAlert = useCallback((_alert: WorkspaceAlert) => undefined, []);

  const runWorkspaceAction = useCallback(
    async (action: () => Promise<void>, {
      markBusy = true,
      reloadRegistry = true,
      missingDirectoryWorkspace: attemptedWorkspace,
    }: WorkspaceActionOptions = {}) => {
      if (markBusy) {
        setBusy(true);
      }
      setError(null);
      try {
        await action();
        if (reloadRegistry) {
          await loadWorkspaceRegistry();
        }
      } catch (caught) {
        if (attemptedWorkspace && isWorkspacePrimaryDirectoryMissingError(caught)) {
          setMissingDirectoryWorkspace(attemptedWorkspace);
        } else {
          setError(errorMessage(caught));
        }
      } finally {
        if (markBusy) {
          setBusy(false);
        }
      }
    },
    [loadWorkspaceRegistry]
  );

  const markHistoryDuplicate = useCallback((type: 'claim' | 'memory' | 'runbook', id: string, parentId: string, expectedRevision: number): void => {
    const workspaceId = snapshot?.workspace.workspaceId;
    if (!workspaceId) return;
    void runAction(() => window.beale.markHistoryDuplicate({ workspaceId, type, id, parentId, expectedRevision }));
  }, [runAction, snapshot?.workspace.workspaceId]);

  const undoHistoryDuplicate = useCallback((type: 'claim' | 'memory' | 'runbook', id: string, expectedRevision: number): void => {
    const workspaceId = snapshot?.workspace.workspaceId;
    if (!workspaceId) return;
    void runAction(() => window.beale.undoHistoryDuplicate({ workspaceId, type, id, expectedRevision }));
  }, [runAction, snapshot?.workspace.workspaceId]);

  const transitionClaimOnBoard = useCallback(async (claim: AppServerFindingSummary, targetMaturity: ClaimBoardMaturity): Promise<void> => {
    const workspaceId = snapshot?.workspace.workspaceId;
    if (!workspaceId) throw new Error('No Beale workspace is open.');
    try {
      applySnapshot(await window.beale.transitionClaimOnBoard({
        workspaceId,
        claimId: claim.id,
        expectedRevision: claim.revision,
        targetMaturity
      }));
    } catch (caught) {
      await loadSnapshot().catch(() => undefined);
      throw caught;
    }
  }, [applySnapshot, loadSnapshot, snapshot?.workspace.workspaceId]);

  const openAppServerRunbook = useCallback((runbookId: string): void => {
    setRightSidenavExpanded(true);
    setSelectedSubagentPath(null);
    setSelectedReportId(null);
    setSelectedReportWorkspaceId(null);
    setSelectedReportDocument(null);
    setReportError(null);
    setSelectedRunbookDocument((current) => current?.runbookId === runbookId ? current : null);
    setRunbookLoading(true);
    setRunbookError(null);
    setSelectedRunbookId(runbookId);
    setRunbookOpenVersion((current) => current + 1);
  }, []);

  const openAppServerReport = useCallback((reportId: string): void => {
    setRightSidenavExpanded(true);
    setSelectedSubagentPath(null);
    setSelectedRunbookId(null);
    setSelectedRunbookDocument(null);
    setRunbookError(null);
    setSelectedReportId(reportId);
    setSelectedReportWorkspaceId(snapshot?.workspace.workspaceId ?? null);
  }, [snapshot?.workspace.workspaceId]);

  const workspaceCatalogKey = workspaceRegistry?.workspaces
    .map((workspace) => `${workspace.id}:${workspace.workspaceId}:${workspace.updatedAt}`)
    .join('|') ?? '';

  useEffect(() => {
    if (!automationsOpen) return undefined;
    let cancelled = false;
    setAutomationsLoading(true);
    setAutomationsError(null);
    void window.beale.listAutomations()
      .then((next) => {
        if (!cancelled) setAutomations(next);
      })
      .catch((caught: unknown) => {
        if (!cancelled) setAutomationsError(errorMessage(caught));
      })
      .finally(() => {
        if (!cancelled) setAutomationsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [automationsOpen, workspaceCatalogKey, snapshot?.version]);

  useEffect(() => {
    if (!workspaceRegistry || !automationScopeWorkspaceId) return;
    if (workspaceRegistry.workspaces.some((workspace) => workspace.workspaceId === automationScopeWorkspaceId)) return;
    setAutomationScopeWorkspaceId(null);
  }, [automationScopeWorkspaceId, workspaceRegistry]);

  useEffect(() => {
    if (!workspaceRegistry || !pluginScopeWorkspaceId) return;
    if (workspaceRegistry.workspaces.some((workspace) => workspace.workspaceId === pluginScopeWorkspaceId)) return;
    setPluginScopeWorkspaceId(null);
    setSelectedPluginId(null);
  }, [pluginScopeWorkspaceId, workspaceRegistry]);

  useEffect(() => {
    if (!selectedAutomationRunId || !selectedAutomationWorkspaceId) return;
    if (automations.some((automation) => automation.runId === selectedAutomationRunId && automation.workspaceId === selectedAutomationWorkspaceId)) return;
    clearRunDetail();
    setSelectedRunId(null);
    setSelectedAutomationRunId(null);
    setSelectedAutomationWorkspaceId(null);
  }, [automations, clearRunDetail, selectedAutomationRunId, selectedAutomationWorkspaceId, setSelectedRunId]);

  const openReportSession = useCallback((report: AppServerReportSummary): void => {
    clearRunDetail();
    setSelectedRunId(null);
    setSelectedReportId(report.id);
    setSelectedReportWorkspaceId(report.workspaceId);
    setSelectedReportDocument(null);
    setSelectedReportOverride(report);
    setWorkspaceDashboardInitialView('reports');
    setReportSessionRunId(null);
    setReportSessionRefreshVersion(0);
    setReportError(null);
    setError(null);
    setSettingsOpen(false);
    setFleetOpen(false);
    setAutomationsOpen(false);
    setPluginsOpen(false);
    setReportsOpen(true);
  }, [clearRunDetail, setSelectedRunId]);

  const startReportTurn = useCallback(async (
    instruction: string,
    modelSelection?: ResearchModelSelection,
    shellSafetyMode?: ShellSafetyMode
  ): Promise<RunRecord> => {
    if (!selectedReportId || !selectedReportWorkspaceId) throw new Error('No report is selected.');
    const report = (selectedReportOverride?.id === selectedReportId && selectedReportOverride.workspaceId === selectedReportWorkspaceId
      ? selectedReportOverride : null) ?? snapshot?.appServerMemory.reports.find((candidate) => (
      candidate.id === selectedReportId && candidate.workspaceId === selectedReportWorkspaceId
    ));
    if (!report) throw new Error('The selected report is no longer available.');
    setBusy(true);
    setError(null);
    try {
      const result = await window.beale.startReportSession({
        workspaceId: report.workspaceId,
        reportId: selectedReportId,
        instruction,
        ...(modelSelection ? { modelSelection } : {}),
        ...(shellSafetyMode ? { shellSafetyMode } : {})
      });
      applySnapshot(result.snapshot);
      setReportSessionRunId(result.runId);
      setSelectedRunId(result.runId);
      const run = result.snapshot.runs.find((row) => row.run.id === result.runId)?.run;
      if (!run) throw new Error('The report editing session started without a run record.');
      return run;
    } catch (caught: unknown) {
      const message = errorMessage(caught);
      setReportError(message);
      setError(message);
      throw new Error(message);
    } finally {
      setBusy(false);
    }
  }, [applySnapshot, selectedReportOverride, selectedReportId, selectedReportWorkspaceId, setSelectedRunId, snapshot?.appServerMemory.reports]);

  const submitReportChange = useCallback(async (instruction: string): Promise<void> => {
    if (!reportSessionRunId) {
      await startReportTurn(instruction);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await window.beale.steerRun({
        type: 'steer',
        runId: reportSessionRunId,
        instruction
      });
      applySnapshot(next);
      setReportSessionRefreshVersion((version) => version + 1);
    } catch (caught: unknown) {
      const message = errorMessage(caught);
      setError(message);
      throw new Error(message);
    } finally {
      setBusy(false);
    }
  }, [applySnapshot, reportSessionRunId, startReportTurn]);

  const selectSubagent = useCallback((path: string): void => {
    setRightSidenavExpanded(true);
    setSelectedRunbookId(null);
    setSelectedRunbookDocument(null);
    setRunbookError(null);
    setSelectedReportId(null);
    setSelectedReportWorkspaceId(null);
    setSelectedReportDocument(null);
    setReportError(null);
    setSelectedSubagentPath(path);
  }, []);

  const backToRunbooks = useCallback((): void => {
    setSelectedRunbookId(null);
    setSelectedRunbookDocument(null);
    setRunbookError(null);
  }, []);

  const backToReports = useCallback((): void => {
    setSelectedReportId(null);
    setSelectedReportWorkspaceId(null);
    setSelectedReportDocument(null);
    setReportError(null);
  }, []);

  const backToSubagents = useCallback((): void => {
    setSelectedSubagentPath(null);
  }, []);

  const refreshOpenAiProvider = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      if (snapshot) {
        const next = await window.beale.refreshOpenAiStatus();
        applySnapshot(next);
      } else {
        setOpenAiStatus(await window.beale.getOpenAiStatus());
      }
      setResearchProviderStatuses(await window.beale.getResearchProviderStatuses());
      setResearchProviderStatusesLoaded(true);
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  }, [applySnapshot, snapshot]);

  const loadResearchProviderStatuses = useCallback(async (): Promise<void> => {
    try {
      setResearchProviderStatuses(await window.beale.getResearchProviderStatuses());
      setResearchProviderStatusesLoaded(true);
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  const loadOpenAiProviderStatus = useCallback(async (): Promise<void> => {
    try {
      setOpenAiStatus(await window.beale.getOpenAiStatus());
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  const loadResearchProviderModelCatalog = useCallback(async (): Promise<void> => {
    try {
      setResearchProviderModelCatalog(await window.beale.getResearchProviderModelCatalog());
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);
  const openWorkspaceId = snapshot?.workspace.workspaceId ?? null;

  useEffect(() => {
    if (!newResearchOpen && !automationsOpen && quickChats.length === 0 && !(settingsOpen && settingsSection === 'providers')) return;
    void loadResearchProviderStatuses();
    void loadOpenAiProviderStatus();
  }, [automationsOpen, loadOpenAiProviderStatus, loadResearchProviderStatuses, newResearchOpen, quickChats.length, settingsOpen, settingsSection]);

  useEffect(() => {
    if (!openWorkspaceId && !newResearchOpen && !automationsOpen && !reportsOpen && quickChats.length === 0 && !selectedRunId && !(settingsOpen && settingsSection === 'providers')) return;
    void loadResearchProviderModelCatalog();
  }, [automationsOpen, loadResearchProviderModelCatalog, newResearchOpen, openWorkspaceId, quickChats.length, reportsOpen, selectedRunId, settingsOpen, settingsSection]);

  useEffect(() => {
    if (!settingsOpen || !researchProviderStatuses.some((provider) => provider.loginInProgress)) return;
    let inFlight = false;
    const poll = async (): Promise<void> => {
      if (inFlight) return;
      inFlight = true;
      try {
        await loadResearchProviderStatuses();
      } finally {
        inFlight = false;
      }
    };
    const timer = window.setInterval(() => void poll(), 2_000);
    return () => window.clearInterval(timer);
  }, [loadResearchProviderStatuses, researchProviderStatuses, settingsOpen]);

  const openAiConfigured = openAiStatus?.configured ?? snapshot?.openAi.configured ?? false;
  const openAiAuthenticationRunning = openAiStatus?.loginInProgress ?? false;
  useEffect(() => {
    if (!settingsOpen || settingsSection !== 'providers' || !openAiOAuthResult?.started || (openAiConfigured && !openAiAuthenticationRunning)) return;
    void loadOpenAiProviderStatus();
    const timer = window.setInterval(() => void loadOpenAiProviderStatus(), 2_000);
    const timeout = window.setTimeout(() => window.clearInterval(timer), 5 * 60_000);
    return () => {
      window.clearInterval(timer);
      window.clearTimeout(timeout);
    };
  }, [loadOpenAiProviderStatus, openAiAuthenticationRunning, openAiConfigured, openAiOAuthResult?.started, settingsOpen, settingsSection]);

  useEffect(() => {
    if (!openAiOAuthResult || !isSubscriptionAuthenticationConfirmed(openAiStatus)) return;
    setOpenAiOAuthResult(null);
  }, [openAiOAuthResult, openAiStatus]);

  useEffect(() => {
    setResearchProviderOAuthResults((current) =>
      clearConfirmedProviderOAuthResults(current, researchProviderStatuses));
  }, [researchProviderStatuses]);

  const setDefaultProviderId = useCallback(async (providerId: ResearchModelProviderId | null): Promise<void> => {
    setError(null);
    try {
      setProviderSettings(await window.beale.setDefaultProviderId(providerId));
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  const saveCustomProvider = useCallback(async (input: { id?: `custom-${string}`; name: string; baseUrl: string; contextWindow?: number; maxTokens?: number; apiKey?: string }): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      setProviderSettings(await window.beale.saveCustomProvider(input));
      await loadResearchProviderModelCatalog();
    } catch (caught) {
      setError(errorMessage(caught));
      throw caught;
    } finally { setBusy(false); }
  }, [loadResearchProviderModelCatalog]);

  const removeCustomProvider = useCallback(async (providerId: `custom-${string}`): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      setProviderSettings(await window.beale.removeCustomProvider(providerId));
      await loadResearchProviderModelCatalog();
    } catch (caught) {
      setError(errorMessage(caught));
      throw caught;
    } finally { setBusy(false); }
  }, [loadResearchProviderModelCatalog]);

  const setProviderModelDefaults = useCallback(async (
    providerId: ResearchModelProviderId,
    defaults: ProviderModelDefaults
  ): Promise<void> => {
    setError(null);
    try {
      setProviderSettings(await window.beale.setProviderModelDefaults(providerId, defaults));
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  const setProviderOptionalModelEnabled = useCallback(async (
    providerId: ResearchModelProviderId,
    modelId: string,
    enabled: boolean
  ): Promise<void> => {
    setError(null);
    try {
      setProviderSettings(await window.beale.setProviderOptionalModelEnabled(providerId, modelId, enabled));
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  const setProviderCyberPolicyRiskAcknowledged = useCallback(async (
    providerId: ResearchModelProviderId,
    acknowledged: boolean
  ): Promise<void> => {
    setError(null);
    try {
      setProviderSettings(await window.beale.setProviderCyberPolicyRiskAcknowledged(providerId, acknowledged));
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  const setProviderPreferredAuthenticationMethod = useCallback(async (
    providerId: ResearchModelProviderId,
    method: ProviderAuthenticationMethod
  ): Promise<void> => {
    setError(null);
    try {
      setProviderSettings(await window.beale.setProviderPreferredAuthenticationMethod(providerId, method));
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  const setProviderContextSize = useCallback(async (
    providerId: ResearchModelProviderId,
    contextSize: ProviderContextSize
  ): Promise<void> => {
    setError(null);
    try {
      setProviderSettings(await window.beale.setProviderContextSize(providerId, contextSize));
    } catch (caught) {
      setError(errorMessage(caught));
    }
  }, []);

  const startOpenAiOAuth = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const result = await window.beale.startOpenAiOAuth();
      setOpenAiOAuthResult(result);
      setOpenAiStatus(await window.beale.getOpenAiStatus());
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  }, []);

  const startResearchProviderOAuth = useCallback(async (providerId: ResearchProviderId) => {
    setBusy(true);
    setError(null);
    setResearchProviderOAuthResults((current) => ({
      ...current,
      [providerId]: {
        providerId,
        started: true,
        command: `appServer auth login ${providerId}`,
        detail: `Starting ${providerId === 'anthropic' ? 'Claude.ai subscription' : providerId === 'zai' ? 'Z.ai subscription' : 'provider'} authentication…`,
        verificationUri: null,
        userCode: null,
        instructions: null
      }
    }));
    try {
      const result = await window.beale.startResearchProviderOAuth(providerId);
      setResearchProviderOAuthResults((current) => ({ ...current, [providerId]: result }));
      setResearchProviderStatuses(await window.beale.getResearchProviderStatuses());
      setResearchProviderStatusesLoaded(true);
    } catch (caught) {
      setResearchProviderOAuthResults((current) => {
        const next = { ...current };
        delete next[providerId];
        return next;
      });
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  }, []);

  const reloadProviderAuthentication = useCallback(async (): Promise<void> => {
    setOpenAiStatus(await window.beale.getOpenAiStatus());
    setResearchProviderStatuses(await window.beale.getResearchProviderStatuses());
    setResearchProviderStatusesLoaded(true);
  }, []);

  const forgetProviderSubscription = useCallback(async (providerId: ResearchModelProviderId): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      setProviderSettings(await window.beale.forgetProviderSubscription(providerId));
      await reloadProviderAuthentication();
      if (providerId === 'openai-codex') {
        setOpenAiOAuthResult(null);
      } else {
        setResearchProviderOAuthResults((current) => {
          if (!(providerId in current)) return current;
          const next = { ...current };
          delete next[providerId as ResearchProviderId];
          return next;
        });
      }
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  }, [reloadProviderAuthentication]);

  const removeProvider = useCallback(async (providerId: ResearchModelProviderId): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      setProviderSettings(await window.beale.removeProvider(providerId));
      await reloadProviderAuthentication();
      if (providerId === 'openai-codex') {
        setOpenAiOAuthResult(null);
      } else {
        setResearchProviderOAuthResults((current) => {
          if (!(providerId in current)) return current;
          const next = { ...current };
          delete next[providerId as ResearchProviderId];
          return next;
        });
      }
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  }, [reloadProviderAuthentication]);

  const configureProviderApiKey = useCallback(async (providerId: ResearchModelProviderId, apiKey: string): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      setProviderSettings(await window.beale.configureProviderApiKey(providerId, apiKey));
      await reloadProviderAuthentication();
    } catch (caught) {
      setError(errorMessage(caught));
      throw caught;
    } finally {
      setBusy(false);
    }
  }, [reloadProviderAuthentication]);

  const removeProviderApiKey = useCallback(async (providerId: ResearchModelProviderId): Promise<void> => {
    setBusy(true);
    setError(null);
    try {
      setProviderSettings(await window.beale.removeProviderApiKey(providerId));
      await reloadProviderAuthentication();
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setBusy(false);
    }
  }, [reloadProviderAuthentication]);

  const {
    addWorkspace,
    openRegisteredWorkspace,
    openResearchSession,
    removeRegisteredWorkspace,
    submitWorkspaceOnboarding,
    applyOnboardingResearchKit,
    lookupHackerOneScope,
    lookupBugcrowdScope
  } = useWorkspaceActions({
    snapshot,
    selectedRunId,
    workspaceDraft,
    runWorkspaceAction,
    applySnapshot,
    clearRunDetail,
    setSelectedRunId,
    setWorkspaceDraft,
    setWorkspaceOnboardingProgress
  });
  const beginWorkspaceCreation = useCallback((): void => {
    setError(null);
    setWorkspaceDashboardViewName('Settings');
    closeNewResearch();
    addWorkspace();
  }, [addWorkspace, closeNewResearch]);
  const openWorkspaceFromSidebar = useCallback((workspace: WorkspaceRegistryEntry): void => {
    closeWorkspaceOnboarding();
    closeNewResearch();
    setReportsOpen(false);
    setAutomationsOpen(false);
    setPluginsOpen(false);
    openRegisteredWorkspace(workspace);
  }, [closeNewResearch, closeWorkspaceOnboarding, openRegisteredWorkspace]);
  const openResearchSessionFromSidebar = useCallback((workspace: WorkspaceRegistryEntry, session: ResearchSessionSummary): void => {
    closeWorkspaceOnboarding();
    closeNewResearch();
    setReportsOpen(false);
    setAutomationsOpen(false);
    setPluginsOpen(false);
    openResearchSession(workspace, session);
  }, [closeNewResearch, closeWorkspaceOnboarding, openResearchSession]);
  const importWorkspace = useCallback((): void => {
    void (async () => {
      try {
        const selection = await window.beale.selectWorkspace('open');
        const workspacePath = selection.path;
        if (selection.canceled || !workspacePath) return;
        await runWorkspaceAction(async () => {
          const next = await window.beale.openWorkspace(workspacePath);
          closeWorkspaceOnboarding();
          closeNewResearch();
          clearRunDetail();
          setSelectedRunId(null);
          setReportsOpen(false);
          setAutomationsOpen(false);
          setPluginsOpen(false);
          applySnapshot(next);
          setSelectedRunId(null);
        });
      } catch (caught) {
        setError(errorMessage(caught));
      }
    })();
  }, [applySnapshot, clearRunDetail, closeNewResearch, closeWorkspaceOnboarding, runWorkspaceAction, setSelectedRunId]);
  const removeActiveWorkspace = useCallback(async (): Promise<void> => {
    const workspaceId = snapshot?.workspace.workspaceId;
    const workspace = workspaceRegistry?.workspaces.find((candidate) => candidate.workspaceId === workspaceId);
    if (!workspace) throw new Error('The active workspace registry entry is unavailable.');
    await removeRegisteredWorkspace(workspace);
  }, [removeRegisteredWorkspace, snapshot?.workspace.workspaceId, workspaceRegistry?.workspaces]);
  const removeMissingDirectoryWorkspace = useCallback((): void => {
    const workspace = missingDirectoryWorkspace;
    if (!workspace) return;
    void runWorkspaceAction(async () => {
      applySnapshot(await window.beale.removeRegisteredWorkspace(workspace.id));
      setMissingDirectoryWorkspace(null);
    });
  }, [applySnapshot, missingDirectoryWorkspace, runWorkspaceAction]);

  useEffect(() => window.beale.onNativeMenuAction(() => beginWorkspaceCreation()), [beginWorkspaceCreation]);

  const handleSessionAction = useCallback(
    (action: SteeringAction): void => {
      if (!permissionAllowsSteeringAction(permissionSettings, action)) {
        setError('Enable Danger Mode in Agent Settings before selecting it for a research session.');
        return;
      }
      if ((action.type === 'pause' || action.type === 'stop') && action.runId === selectedRunId
        && action.runId.startsWith('track_') && snapshot) {
        setBusy(true);
        setError(null);
        void pauseResearchTrackSessionAction({ action: action.type, workspaceId: snapshot.workspace.workspaceId,
          sessionId: action.runId, api: window.beale })
          .catch((caught: unknown) => setError(errorMessage(caught)))
          .finally(() => setBusy(false));
        return;
      }
      void runAction(() => window.beale.steerRun(action));
    },
    [permissionSettings.dangerModeEnabled, runAction, selectedRunId, snapshot]
  );

  const handleSteerInstruction = useCallback(
    (runId: string, instruction: string, modelSelection: ResearchModelSelection): void => {
      if (runId === selectedRunId && (selectedRunState === 'blocked' || selectedRunState === 'completed'
        || selectedRunState === 'failed' || selectedRunState === 'stopped')) {
        setContinuationPendingRunId(runId);
      }
      setBusy(true);
      setError(null);
      void window.beale.steerRun({ type: 'steer', runId, instruction, modelSelection })
        .then(applySnapshot)
        .catch((caught: unknown) => {
          setContinuationPendingRunId(null);
          setError(errorMessage(caught));
        })
        .finally(() => setBusy(false));
    },
    [applySnapshot, selectedRunId, selectedRunState]
  );

  const applyReportSessionAction = useCallback(async (action: SteeringAction): Promise<RunRecord | null> => {
    if (!permissionAllowsSteeringAction(permissionSettings, action)) {
      throw new Error('Enable Danger Mode in Agent Settings before selecting it for a research session.');
    }
    setBusy(true);
    setError(null);
    try {
      const next = await window.beale.steerRun(action);
      applySnapshot(next);
      setReportSessionRefreshVersion((version) => version + 1);
      return next.runs.find((row) => row.run.id === action.runId)?.run ?? null;
    } catch (caught: unknown) {
      const message = errorMessage(caught);
      setReportError(message);
      setError(message);
      throw new Error(message);
    } finally {
      setBusy(false);
    }
  }, [applySnapshot, permissionSettings]);

  const activeRunDetail = activeRunDetailForSelection(runDetail, selectedRunId);
  const activeRunHeaderDetail = useMemo(() => activeRunDetail ? {
    run: {
      id: activeRunDetail.run.id,
      title: activeRunDetail.run.title,
      promptMarkdown: activeRunDetail.run.promptMarkdown
    }
  } : null, [activeRunDetail?.run.id, activeRunDetail?.run.promptMarkdown, activeRunDetail?.run.title]);
  const presentedActiveRunDetail = useMemo(
    () => runningResearchTrackDetail(activeRunDetail, selectedResearchTrackRunning),
    [activeRunDetail, selectedResearchTrackRunning]
  );
  const deferredActiveRunDetail = useDeferredValue(presentedActiveRunDetail);
  const renderedRunDetail = useMemo(
    () => deferredResearchTrackDetail(presentedActiveRunDetail, deferredActiveRunDetail),
    [presentedActiveRunDetail, deferredActiveRunDetail]
  );
  const localSurfaceFocus = useMemo(() => (
    !newResearchOpen && !reportsOpen && !workspaceDraft && selectedRunId
      && presentedActiveRunDetail?.run.status === 'active' && renderedRunDetail?.run.id === selectedRunId
      ? surfaceFocusSummary(renderedRunDetail.traceEvents ?? [], 'active')
      : null
  ), [newResearchOpen, reportsOpen, workspaceDraft, selectedRunId, presentedActiveRunDetail?.run.status, renderedRunDetail]);
  const activeResearchProfile = selectedRunId
    ? activeRunDetail?.researchProfile?.profile ?? null
    : snapshot?.researchProfile.profile ?? null;
  const activeResearchFeatures = researchProfileFeatureAvailability(activeResearchProfile);
  const researchDetailsAvailable = !newResearchOpen
    && (selectedRunId ? activeRunDetail !== null : snapshot !== null)
    && (selectedRunId
      ? hasResearchProfileDetailFeatures(activeResearchProfile)
      : activeResearchFeatures.memory || activeResearchFeatures.runbooks || activeResearchFeatures.reports);

  const selectedShellApproval = useMemo(() => {
    if (!snapshot) return pendingShellApproval(activeRunDetail);
    return snapshot.pendingShellApprovals.find((approval) => approval.runId === selectedRunId) ?? null;
  }, [activeRunDetail?.policyEvents, selectedRunId, snapshot]);
  const inlineApproval = isInlineApproval(selectedShellApproval)
    ? selectedShellApproval
    : null;
  const activeManualShellApproval = useMemo(() => {
    if (selectedShellApproval && !isInlineApproval(selectedShellApproval)) return selectedShellApproval;
    return snapshot?.pendingShellApprovals.find((approval) => !isInlineApproval(approval)) ?? null;
  }, [selectedShellApproval, snapshot?.pendingShellApprovals]);
  const activeShellApproval = inlineApproval ?? activeManualShellApproval;
  useEffect(() => {
    if (shellApprovalDecisionRef.current === activeShellApproval?.id) return;
    shellApprovalDecisionRef.current = null;
    setShellApprovalDecisionInFlight(null);
  }, [activeShellApproval?.id]);

  const handleShellApprovalDecision = useCallback((
    approval: ApprovalRecord,
    decision: PolicyReviewDecision
  ): void => {
    if (shellApprovalDecisionRef.current) return;
    shellApprovalDecisionRef.current = approval.id;
    setShellApprovalDecisionInFlight(approval.id);
    void (async () => {
      setBusy(true);
      setError(null);
      try {
        const next = await window.beale.steerRun({
          type: 'review_shell_command',
          workspacePath: shellApprovalWorkspacePath(approval),
          runId: approval.runId,
          approvalId: approval.id,
          decision
        });
        if (next) applySnapshot(next);
      } catch (caught) {
        shellApprovalDecisionRef.current = null;
        setShellApprovalDecisionInFlight(null);
        setError(errorMessage(caught));
      } finally {
        setBusy(false);
      }
    })();
  }, [applySnapshot]);
  const researchPanelMemory = selectedRunId
    ? activeRunDetail?.appServerMemory ?? null
    : snapshot?.appServerMemory ?? null;
  const selectedRunbook = useMemo(
    () => researchPanelMemory?.runbooks.find((runbook) => runbook.id === selectedRunbookId) ?? null,
    [researchPanelMemory?.runbooks, selectedRunbookId]
  );
  const selectedReport = useMemo(
    () => {
      if (!selectedReportId) return null;
      const memoryReport = researchPanelMemory?.reports.find((report) => (
        report.id === selectedReportId && (!selectedReportWorkspaceId || report.workspaceId === selectedReportWorkspaceId)
      )) ?? null;
      if (!reportsOpen) return memoryReport;
      const snapshotReport = snapshot?.appServerMemory.reports.find((report) => (
        report.id === selectedReportId && report.workspaceId === selectedReportWorkspaceId
      )) ?? null;
      const storedReport = snapshotReport ?? memoryReport;
      const override = selectedReportOverride?.id === selectedReportId
        && selectedReportOverride.workspaceId === selectedReportWorkspaceId ? selectedReportOverride : null;
      return storedReport && (!override || storedReport.revision > override.revision) ? storedReport : override;
    },
    [reportsOpen, researchPanelMemory?.reports, selectedReportId, selectedReportOverride, selectedReportWorkspaceId, snapshot?.appServerMemory.reports]
  );
  const displayedQuickChats = useMemo<QuickChatDescriptor[]>(() => {
    if (settingsOpen || !reportsOpen || !selectedReport) return quickChats;
    const reportRun = snapshot?.runs.find((row) => row.run.id === reportSessionRunId)?.run;
    const title = reportTitleFromMarkdown(selectedReportDocument?.content ?? '', selectedReport.title);
    return [{
      id: `report-edit:${selectedReport.workspaceId}:${selectedReport.id}`,
      kind: 'report-edit',
      title,
      ...(reportSessionRunId ? { runId: reportSessionRunId } : {}),
      ...(reportRun ? { runState: reportRun.status } : {})
    }, ...quickChats];
  }, [quickChats, reportSessionRunId, reportsOpen, selectedReport, selectedReportDocument?.content, settingsOpen, snapshot?.runs]);
  const workspaceDashboardRuns = useMemo(
    () => snapshot?.runs.filter((row) => !isReportResourceRun(row.run)) ?? [],
    [snapshot?.runs]
  );
  useEffect(() => {
    if (!selectedRunbookId || !researchPanelMemory || selectedRunbook) return;
    setSelectedRunbookId(null);
    setSelectedRunbookDocument(null);
    setRunbookError(null);
  }, [researchPanelMemory, selectedRunbook, selectedRunbookId]);
  useEffect(() => () => {
    runbookExecutionRequestRef.current += 1;
  }, [selectedRunbookId]);
  const fetchAppServerRunbook = useCallback((runbookId: string): Promise<AppServerRunbookDocument> => {
    const inFlight = runbookDocumentFetchRef.current;
    if (inFlight?.runbookId === runbookId) return inFlight.request;
    const request = window.beale.getAppServerRunbook(runbookId).finally(() => {
      if (runbookDocumentFetchRef.current?.request === request) runbookDocumentFetchRef.current = null;
    });
    runbookDocumentFetchRef.current = { runbookId, request };
    return request;
  }, []);
  const applyAppServerRunbookDocument = useCallback((document: AppServerRunbookDocument): void => {
    setSelectedRunbookDocument((current) => current && appServerRunbookDocumentUpdateKey(current) === appServerRunbookDocumentUpdateKey(document)
      ? current
      : document);
  }, []);
  useEffect(() => {
    if (!selectedRunbookId) {
      setRunbookLoading(false);
      return undefined;
    }
    let cancelled = false;
    setRunbookLoading(true);
    setRunbookError(null);
    setSelectedRunbookDocument((current) => current?.runbookId === selectedRunbookId ? current : null);
    void fetchAppServerRunbook(selectedRunbookId)
      .then((document) => {
        if (cancelled) return;
        if (document.runbookId !== selectedRunbookId) {
          throw new Error('The app-server returned a different runbook than the one selected.');
        }
        applyAppServerRunbookDocument(document);
        setRunbookLoading(false);
      })
      .catch((caught: unknown) => {
        if (!cancelled) {
          setRunbookError(errorMessage(caught));
          setRunbookLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [applyAppServerRunbookDocument, fetchAppServerRunbook, runbookOpenVersion, selectedRunbook?.revision, selectedRunbookId]);

  const runAppServerRunbook = useCallback(async (
    runbookId: string,
    selection: RunbookExecutionSelection
  ): Promise<void> => {
    const runbook = researchPanelMemory?.runbooks.find((candidate) => candidate.id === runbookId);
    if (!runbook?.sessionId) throw new Error('This runbook is not attached to an executable app-server session.');
    const previousRunId = selectedRunbookDocument?.runbookId === runbookId
      ? selectedRunbookDocument.latestRun?.runId ?? null
      : null;
    const request = runbookExecutionRequestRef.current + 1;
    runbookExecutionRequestRef.current = request;
    setRunbookError(null);
    try {
      const next = await window.beale.steerRun({
        type: 'run_runbook',
        runId: runbook.sessionId,
        runbookId,
        ...(selection.cellId ? { cellId: selection.cellId } : {}),
        ...(selection.startCellId ? { startCellId: selection.startCellId } : {}),
        ...(selection.endCellId ? { endCellId: selection.endCellId } : {})
      });
      applySnapshot(next);
      for (let attempt = 0; attempt < 30 && runbookExecutionRequestRef.current === request; attempt += 1) {
        await new Promise((resolve) => window.setTimeout(resolve, 1_000));
        const document = await fetchAppServerRunbook(runbookId);
        if (runbookExecutionRequestRef.current !== request) return;
        applyAppServerRunbookDocument(document);
        setRunbookLoading(false);
        const latestRun = document.latestRun;
        if (latestRun && latestRun.runId !== previousRunId) {
          if (latestRun.status !== 'running' && latestRun.status !== 'queued') {
            const refreshed = await window.beale.getSnapshot();
            if (refreshed) applySnapshot(refreshed);
          }
          return;
        }
        if (attempt >= 29) {
          throw new Error('app-server acknowledged the request but the runbook did not start. Inspect the session trace for the execution error.');
        }
      }
    } catch (caught: unknown) {
      const message = errorMessage(caught);
      setRunbookError(message);
      throw new Error(message);
    }
  }, [applyAppServerRunbookDocument, applySnapshot, fetchAppServerRunbook, researchPanelMemory?.runbooks, selectedRunbookDocument]);
  useEffect(() => {
    if (!selectedReportId || selectedReport) return;
    setSelectedReportId(null);
    setSelectedReportWorkspaceId(null);
    setSelectedReportDocument(null);
    setReportError(null);
  }, [selectedReport, selectedReportId]);
  useEffect(() => {
    if (!selectedReportId || !selectedReport) {
      setReportLoading(false);
      return undefined;
    }
    let cancelled = false;
    setReportLoading(true);
    setReportError(null);
    setSelectedReportDocument((current) => current?.reportId === selectedReportId ? current : null);
    void window.beale.getAppServerReport({ workspaceId: selectedReport.workspaceId, reportId: selectedReportId })
      .then((document) => {
        if (cancelled || document.reportId !== selectedReportId) return;
        setSelectedReportDocument(document);
      })
      .catch((caught: unknown) => {
        if (!cancelled) setReportError(errorMessage(caught));
      })
      .finally(() => {
        if (!cancelled) setReportLoading(false);
      });
    return () => { cancelled = true; };
  }, [selectedReport?.revision, selectedReport?.workspaceId, selectedReportId]);
  useEffect(() => {
    const artifactId = selectedReport?.submissionPacket?.artifactId;
    if (!selectedReport || !artifactId) {
      setReportSubmissionPacketPath(null);
      return undefined;
    }
    let cancelled = false;
    setReportSubmissionPacketPath((current) => (
      current?.workspaceId === selectedReport.workspaceId
        && current.reportId === selectedReport.id
        && current.artifactId === artifactId
        ? current
        : null
    ));
    void window.beale.getReportSubmissionPacketPath({
      workspaceId: selectedReport.workspaceId,
      reportId: selectedReport.id
    }).then((path) => {
      if (cancelled) return;
      setReportSubmissionPacketPath({
        workspaceId: selectedReport.workspaceId,
        reportId: selectedReport.id,
        artifactId,
        path
      });
    }).catch(() => {
      if (!cancelled) setReportSubmissionPacketPath(null);
    });
    return () => { cancelled = true; };
  }, [selectedReport?.id, selectedReport?.submissionPacket?.artifactId, selectedReport?.workspaceId]);

  const activeTraceEvents = useMemo(
    () => (renderedRunDetail && selectedSubagentPath
      ? devInstrumentation.time('trace.buildDisplayEvents.active', () => buildTraceDisplayEvents(renderedRunDetail), runDetailMetricDetail(renderedRunDetail))
      : []),
    [renderedRunDetail, selectedSubagentPath]
  );
  const mainSessionTraceEvents = useMemo(
    () => renderedRunDetail
      ? devInstrumentation.time('trace.buildDisplayEvents.root', () => buildTraceDisplayEventsForAgentPath(renderedRunDetail, null), runDetailMetricDetail(renderedRunDetail))
      : [],
    [renderedRunDetail]
  );
  const needsSubagentSummaries = Boolean(selectedSubagentPath);
  const activeSubagents = useMemo(
    () => needsSubagentSummaries
      ? subagentSummaries(activeTraceEvents, renderedRunDetail?.run.status, 'commentary', [],
        renderedRunDetail?.attempts.at(-1)?.id, renderedRunDetail?.attempts.at(-1)?.startedAt)
      : [],
    [activeTraceEvents, needsSubagentSummaries, renderedRunDetail?.run.status]
  );
  useEffect(() => {
    if (!renderedRunDetail || !selectedSubagentPath || activeSubagents.some((agent) => agent.path === selectedSubagentPath)) return;
    setSelectedSubagentPath(null);
  }, [activeSubagents, renderedRunDetail, selectedSubagentPath]);
  const [sessionHeatDisplay, setSessionHeatDisplay] = useState(EMPTY_SESSION_HEAT_DISPLAY_STATE);
  const sessionHeatRunId = newResearchOpen ? null : selectedRunId;
  const sessionHeatRunDetail = newResearchOpen ? null : renderedRunDetail;
  useLayoutEffect(() => {
    setSessionHeatDisplay((previous) => sessionHeatDisplayStateForSelection(
      previous,
      sessionHeatRunId,
      sessionHeatRunDetail,
      sessionHeatPreferences
    ));
  }, [sessionHeatPreferences, sessionHeatRunDetail, sessionHeatRunId]);
  const sessionHeat = sessionHeatDisplay.heat;
  const sessionHeatProfile = sessionHeatDisplay.profile ?? snapshot?.researchProfile.profile ?? null;
  const shellStyle = {
    '--sidebar-width': `${sidebarWidth}px`,
    ...sessionHeatPaletteStyle(sessionHeatPaletteForProfile(sessionHeatProfile, sessionHeatPreferences, appearanceTheme))
  } as CSSProperties;
  const windowControlPlatform = windowControlPlatformForState(snapshot, hostEnvironment);
  const headerResearchControlsAvailable = shouldShowHeaderResearchControls({
    researchDetailsAvailable,
    newResearchOpen,
    workspaceOpen: snapshot !== null,
    settingsOpen,
    fleetOpen,
    reportsOpen,
    automationsOpen,
    pluginsOpen
  });
  const shellClassName = `${appShellClassName({
    sessionHeat,
    sessionInProgress: selectedFleetServerId === 'local'
      && selectedRunId !== null
      && (selectedRunState === 'active' || selectedResearchTrackRunning)
      && !newResearchOpen && !settingsOpen && !fleetOpen && !reportsOpen && !automationsOpen && !pluginsOpen,
    platform: windowControlPlatform,
    windowChromeState,
    sidebarCollapsed
  })}${settingsOpen || fleetOpen ? ' settings-open' : ''}`;
  const currentWorkspaceName = snapshot?.activeScope.workspaceName ?? 'No Workspace Selected';
  const automationScopeName = automationScopeWorkspaceId
    ? workspaceRegistry?.workspaces.find((workspace) => workspace.workspaceId === automationScopeWorkspaceId)?.workspaceName ?? 'Workspace'
    : 'All Automations';
  const selectedAutomation = useMemo(
    () => automations.find((automation) => (
      automation.runId === selectedAutomationRunId && automation.workspaceId === selectedAutomationWorkspaceId
    )) ?? null,
    [automations, selectedAutomationRunId, selectedAutomationWorkspaceId]
  );
  const openSettings = useCallback(() => {
    closeWorkspaceOnboarding();
    closeNewResearch();
    setSettingsSection('general');
    setFleetOpen(false);
    setSettingsOpen(true);
  }, [closeNewResearch, closeWorkspaceOnboarding]);
  const openFleet = useCallback((): void => {
    closeWorkspaceOnboarding();
    closeNewResearch();
    clearRunDetail();
    setSelectedRunId(null);
    setReportsOpen(false);
    setAutomationsOpen(false);
    setPluginsOpen(false);
    setSettingsOpen(false);
    setFleetOpen(true);
  }, [clearRunDetail, closeNewResearch, closeWorkspaceOnboarding, setSelectedRunId]);
  const openHome = useCallback((): void => {
    closeWorkspaceOnboarding();
    closeNewResearch();
    clearRunDetail();
    setSelectedRunId(null);
    setReportsOpen(false);
    setAutomationsOpen(false);
    setPluginsOpen(false);
    setSettingsOpen(false);
    setFleetOpen(false);
    setRightSidenavExpanded(false);
  }, [clearRunDetail, closeNewResearch, closeWorkspaceOnboarding, setSelectedRunId]);
  const openProfiling = useCallback(() => {
    flushProfilingReport();
    setProfilingOpen(true);
  }, [flushProfilingReport]);
  const toggleRightSidenav = useCallback(() => setRightSidenavExpanded((current) => !current), []);
  const changeResearchDetailsOpen = useCallback((expanded: boolean): void => {
    setRightSidenavExpanded(headerResearchControlsAvailable && expanded);
  }, [headerResearchControlsAvailable]);
  const decideInlineShellApproval = useCallback((decision: PolicyReviewDecision): void => {
    if (inlineApproval) handleShellApprovalDecision(inlineApproval, decision);
  }, [handleShellApprovalDecision, inlineApproval]);
  const closeProfiling = useCallback(() => setProfilingOpen(false), []);
  const openNewResearch = useCallback((openSchedule: boolean) => {
    setNewResearchTrackMode(false);
    setNewResearchInitialInput(null);
    closeWorkspaceOnboarding();
    if (reportsOpen) {
      clearRunDetail();
      setSelectedRunId(null);
      setReportSessionRunId(null);
      setSelectedReportId(null);
      setSelectedReportWorkspaceId(null);
      setSelectedReportDocument(null);
    }
    setReportsOpen(false);
    setAutomationsOpen(false);
    setPluginsOpen(false);
    setSettingsOpen(false);
    setFleetOpen(false);
    setOpenScheduleOnNewResearch(openSchedule);
    setNewResearchOpen(true);
  }, [clearRunDetail, closeWorkspaceOnboarding, reportsOpen, setSelectedRunId]);
  const startNewResearch = useCallback(() => openNewResearch(false), [openNewResearch]);
  const openStartTrack = useCallback(() => {
    openNewResearch(false);
    setNewResearchTrackMode(true);
  }, [openNewResearch]);
  const openNewResearchForWorkspace = useCallback((workspace: WorkspaceRegistryEntry, openSchedule: boolean): void => {
    if (snapshot?.workspace.workspacePath === workspace.workspacePath) {
      openNewResearch(openSchedule);
      return;
    }
    void runWorkspaceAction(async () => {
      clearRunDetail();
      setSelectedRunId(null);
      applySnapshot(await window.beale.openRegisteredWorkspace(workspace.id));
      setSelectedRunId(null);
      openNewResearch(openSchedule);
    }, { reloadRegistry: false, missingDirectoryWorkspace: workspace });
  }, [applySnapshot, clearRunDetail, openNewResearch, runWorkspaceAction, setSelectedRunId, snapshot?.workspace.workspacePath]);
  const startNewResearchForWorkspace = useCallback((workspace: WorkspaceRegistryEntry): void => {
    openNewResearchForWorkspace(workspace, false);
  }, [openNewResearchForWorkspace]);
  const handleResearchStarted = useCallback(
    (run: RunRecord): void => {
      primeRunDetail(run);
      setSelectedRunId(run.id);
      setNewResearchOpen(false);
      setNewResearchInitialInput(null);
    },
    [primeRunDetail, setSelectedRunId]
  );
  const cancelNewResearch = useCallback((): void => {
    closeNewResearch();
  }, [closeNewResearch]);
  const openWorkspaceDashboardSession = useCallback(
    (runId: string): void => {
      closeNewResearch();
      setReportsOpen(false);
      setAutomationsOpen(false);
      setPluginsOpen(false);
      clearRunDetail();
      setSelectedRunId(runId);
    },
    [clearRunDetail, closeNewResearch, setSelectedRunId]
  );
  const openAutomations = useCallback((): void => {
    closeWorkspaceOnboarding();
    closeNewResearch();
    setSettingsOpen(false);
    setFleetOpen(false);
    clearRunDetail();
    setSelectedRunId(null);
    setSelectedAutomationRunId(null);
    setSelectedAutomationWorkspaceId(null);
    setAutomationScopeWorkspaceId(snapshot?.workspace.workspaceId ?? null);
    setAutomationsError(null);
    setReportsOpen(false);
    setPluginsOpen(false);
    setAutomationsOpen(true);
  }, [clearRunDetail, closeNewResearch, closeWorkspaceOnboarding, setSelectedRunId, snapshot?.workspace.workspaceId]);
  const selectAutomation = useCallback((automation: AutomationSummary | null): void => {
    clearRunDetail();
    setSelectedRunId(null);
    if (!automation) {
      setSelectedAutomationRunId(null);
      setSelectedAutomationWorkspaceId(null);
      return;
    }
    const workspace = workspaceRegistry?.workspaces.find((candidate) => candidate.workspaceId === automation.workspaceId);
    if (!workspace) {
      setError(`The workspace for “${automation.title}” is no longer registered.`);
      return;
    }
    setSelectedAutomationRunId(automation.runId);
    setSelectedAutomationWorkspaceId(automation.workspaceId);
    if (snapshot?.workspace.workspacePath === workspace.workspacePath) {
      setSelectedRunId(automation.runId);
      return;
    }
    void runWorkspaceAction(async () => {
      const next = await window.beale.openRegisteredWorkspace(workspace.id);
      applySnapshot(next, automation.runId);
    }, { markBusy: false, reloadRegistry: false, missingDirectoryWorkspace: workspace });
  }, [applySnapshot, clearRunDetail, runWorkspaceAction, setSelectedRunId, snapshot?.workspace.workspacePath, workspaceRegistry?.workspaces]);
  const newResearchContent = newResearchOpen && snapshot ? (
    <StartRunForm
      trackMode={newResearchTrackMode}
      onTrackStarted={(track) => {
        closeNewResearch();
        navigateToResearchTrack(track, null);
      }}
      initialInput={newResearchInitialInput ?? undefined}
      presentation="session"
      autoOpenSchedule={openScheduleOnNewResearch}
      snapshot={snapshot}
      openAiStatus={snapshot.openAi ?? openAiStatus}
      defaultProviderId={providerSettings?.defaultProviderId}
      defaultShellSafetyMode={permissionSettings.defaultShellSafetyMode}
      guardianEnabled={permissionSettings.guardianEnabled}
      providerModelDefaults={providerSettings?.modelDefaults}
      providerPolicyRiskAcknowledgements={providerSettings?.cyberPolicyRiskAcknowledgements}
      researchProviderStatuses={researchProviderStatuses}
      providerModelCatalog={enabledResearchProviderModelCatalog}
      researchGoalSuggestions={researchGoalSuggestionState.suggestions}
      researchGoalSuggestionsLoading={researchGoalSuggestionState.loading}
      researchGoalSuggestionErrors={researchGoalSuggestionState.errors}
      showSuggestions={!newResearchTrackMode && suggestionPreferences.newResearchPromptSuggestionsEnabled}
      busy={busy}
      runAction={runAction}
      onCancel={cancelNewResearch}
      onLoadResearchGoalSuggestions={researchGoalSuggestionState.load}
      onSelectResearchGoalSuggestion={researchGoalSuggestionState.consume}
      onRetryResearchGoalSuggestions={researchGoalSuggestionState.retry}
      onStarted={handleResearchStarted}
    />
  ) : null;
  const navigationDestination = resolveAppNavigationDestination({ settingsOpen, fleetOpen, automationsOpen, pluginsOpen });
  const selectedFleetServer = fleetServers.find((server) => server.id === selectedFleetServerId);
  const openSelectedQuickChat = (): void => {
    if (selectedFleetServerId === 'local') openQuickChat();
    else setRemoteQuickChats((current) => [...current, { id: `remote-quick-chat-${crypto.randomUUID()}`, serverId: selectedFleetServerId }]);
  };
  const remoteWorkspace = selectedFleetServerId === 'local'
    ? null : remoteCatalog?.workspaces.find((workspace) => workspace.workspaceId === remoteWorkspaceId);
  const remoteSession = selectedFleetServerId === 'local'
    ? null : remoteCatalog?.sessions.find((session) => session.id === remoteSessionId && session.workspaceId === remoteWorkspaceId);
  const remoteRegistry = selectedFleetServerId === 'local' ? null : remoteWorkspaceRegistry(remoteCatalog, selectedFleetServerId);
  const serverSelector = <SidebarServerSelector servers={fleetServers} value={selectedFleetServerId} collapsed={sidebarCollapsed} onChange={(serverId) => {
    setSelectedFleetServerId(serverId);
    setRemoteCatalog(null);
    setRemoteWorkspaceId(null);
    setRemoteSessionId(null);
    setRemoteResearchOpen(false);
  }} statuses={fleetServerStatuses} onRefreshStatuses={refreshFleetServerStatuses} onAdd={() => {
    setFleetSelection({ section: 'app-servers', itemId: 'new' });
    openFleet();
  }} />;
  return (
    <div
      ref={appShellRef}
      className={shellClassName}
      data-theme={appearanceTheme}
      style={shellStyle}
    >
      <TopBar
        sidebarCollapsed={sidebarCollapsed}
        workspaceOpen={selectedFleetServerId === 'local' && Boolean(snapshot)}
        newResearchLabel={snapshot?.researchProfile.profile.presentation?.newResearchLabel ?? 'New Research'}
        newResearchDisabled={busy}
        workspaces={selectedFleetServerId === 'local' ? workspaceRegistry?.workspaces ?? [] : remoteRegistry?.workspaces ?? []}
        workspaceRegistryLoading={selectedFleetServerId === 'local' ? startupPhase === 'shell' || startupPhase === 'registry' : !remoteCatalog}
        rightSidenavAvailable={selectedFleetServerId === 'local' && headerResearchControlsAvailable}
        rightSidenavExpanded={selectedFleetServerId === 'local' && rightSidenavExpanded && headerResearchControlsAvailable}
        contextualTitleVisible={!settingsOpen && !fleetOpen && !reportsOpen && !automationsOpen && !pluginsOpen
          && (selectedFleetServerId === 'local'
            ? Boolean(snapshot || workspaceDraft || newResearchOpen)
            : Boolean(remoteWorkspaceId || remoteResearchOpen))}
        staticContextTitle={settingsOpen
          ? { primary: 'Agent Settings', secondary: settingsSectionLabel(settingsSection), icon: settingsSectionHeaderIcon(settingsSection) }
          : fleetOpen
            ? { primary: 'Fleet', secondary: 'Machine Management', icon: 'fleet' }
          : reportsOpen
            ? {
                primary: 'Reports',
                secondary: selectedReport
                  ? reportTitleFromMarkdown(selectedReportDocument?.content ?? '', selectedReport.title)
                  : currentWorkspaceName,
                icon: 'reporting'
              }
            : automationsOpen
              ? { primary: 'Automations', secondary: selectedAutomation?.title ?? automationScopeName, icon: 'automations' }
              : pluginsOpen
                ? { primary: 'Plugins', secondary: 'Installed Plugins', icon: 'plugins' }
              : null}
        platform={windowControlPlatform}
        workspaceName={selectedFleetServerId !== 'local'
          ? remoteWorkspace?.name ?? 'No Workspace Selected'
          : workspaceDraft && !newResearchOpen ? 'New Workspace' : currentWorkspaceName}
        workspaceViewTitle={selectedFleetServerId !== 'local' ? remoteResearchOpen ? remoteTrackMode ? 'Start Track' : 'New Research' : remoteWorkspaceId && !remoteSessionId ? remoteWorkspaceViewName : null : newResearchOpen
          ? newResearchTrackMode ? 'Start Track' : snapshot?.researchProfile.profile.presentation?.newResearchLabel ?? 'New Research'
          : workspaceDraft ? workspaceDashboardViewName : (snapshot && !selectedRunId ? workspaceDashboardViewName : null)}
        activeRunDetail={selectedFleetServerId !== 'local' && remoteSession
          ? { run: { id: remoteSession.id, title: remoteSession.title, promptMarkdown: remoteSession.prompt } }
          : activeRunHeaderDetail}
        surfaceFocus={selectedFleetServerId !== 'local'
          ? remoteSurfaceFocus?.serverId === selectedFleetServerId
            && remoteSurfaceFocus.workspaceId === remoteWorkspaceId
            && remoteSurfaceFocus.sessionId === remoteSessionId
            && !remoteResearchOpen && remoteRunStatus(remoteSession?.status ?? '') === 'active'
            ? remoteSurfaceFocus.summary : null
          : localSurfaceFocus}
        profilingEnabled={profilingState?.enabled ?? false}
        workspaceEditors={selectedFleetServerId === 'local' && headerResearchControlsAvailable ? workspaceEditors : null}
        onOpenProfiling={openProfiling}
        onOpenWorkspaceInEditor={openWorkspaceInEditor}
        onAddWorkspace={beginWorkspaceCreation}
        onToggleRightSidenav={toggleRightSidenav}
        onToggleSidebar={toggleSidebar}
        onStartNewResearch={selectedFleetServerId === 'local' ? startNewResearch : () => { if (remoteWorkspaceId) { setRemoteSessionId(null); setRemoteTrackMode(false); setRemoteResearchOpen(true); } }}
        onStartNewResearchForWorkspace={selectedFleetServerId === 'local' ? startNewResearchForWorkspace : (workspace) => {
          setRemoteWorkspaceId(workspace.workspaceId);
          setRemoteSessionId(null);
          setRemoteResearchOpen(true);
          setRemoteTrackMode(false);
        }}
        onOpenQuickChat={openSelectedQuickChat}
      />
      <AppNavigationRail
        active={navigationDestination}
        selectedServerId={selectedFleetServerId}
        providerAuthRevision={`${openAiStatus?.subscriptionConfigured ?? ''}:${researchProviderStatuses.map((provider) => `${provider.id}:${provider.subscriptionConfigured}`).join(',')}`}
        onOpenHome={openHome}
        onOpenAutomations={openAutomations}
        onOpenPlugins={openPlugins}
        onOpenSettings={openSettings}
        onOpenFleet={openFleet}
      />
      {settingsOpen ? (
        <SettingsSidebar
          collapsed={sidebarCollapsed}
          section={settingsSection}
          error={error}
          onChangeSection={setSettingsSection}
          onResizePointerDown={beginSidebarResize}
        />
      ) : fleetOpen ? (
        <FleetSidebar
          collapsed={sidebarCollapsed}
          state={fleetConfiguration}
          statuses={fleetServerStatuses}
          selection={fleetSelection}
          error={fleetConfigurationError}
          onSelect={setFleetSelection}
          onSetBrowserEnabled={async (browserId, enabled) => {
            setFleetConfiguration(await window.beale.configureFleet({ action: 'set-browser-enabled', browserId, enabled }));
          }}
          onResizePointerDown={beginSidebarResize}
        />
      ) : pluginsOpen ? (
        <PluginsSidebar
          state={agentPluginState}
          selectedWorkspace={workspaceRegistry?.workspaces.find((workspace) => workspace.workspaceId === pluginScopeWorkspaceId) ?? null}
          selectedPluginId={selectedPluginId}
          collapsed={sidebarCollapsed}
          loading={agentPluginsLoading}
          busy={agentPluginsBusy}
          error={agentPluginsError}
          repositoryUrl={pluginRepositoryUrl}
          onRepositoryUrlChange={setPluginRepositoryUrl}
          onAddFilesystem={addAgentPluginFromFilesystem}
          onAddRepository={addAgentPluginFromRepository}
          onInstall={installAgentPlugin}
          onSetEnabled={setVisibleAgentPluginEnabled}
          onSelectPlugin={setSelectedPluginId}
          onResizePointerDown={beginSidebarResize}
        />
      ) : selectedFleetServerId !== 'local' ? (
        <RemoteFleetSidebar
          catalog={remoteCatalog} serverName={selectedFleetServer?.name ?? 'Remote'}
          selectedWorkspaceId={remoteWorkspaceId} selectedSessionId={remoteSessionId}
          automations={automationsOpen} collapsed={sidebarCollapsed} serverSelector={serverSelector}
          onSelect={(workspaceId, sessionId) => { setRemoteWorkspaceId(workspaceId); setRemoteSessionId(sessionId); setRemoteResearchOpen(false); }}
          onStartNewResearch={(workspaceId) => { setRemoteWorkspaceId(workspaceId); setRemoteSessionId(null); setRemoteTrackMode(false); setRemoteResearchOpen(true); }}
          onOpenQuickChat={openSelectedQuickChat}
          onResizePointerDown={beginSidebarResize}
        />
      ) : automationsOpen ? (
        <AutomationsSidebar
          serverSelector={serverSelector}
          automations={automations}
          workspaces={workspaceRegistry?.workspaces ?? []}
          workspaceOpen={Boolean(snapshot)}
          workspaceRegistryLoading={startupPhase === 'shell' || startupPhase === 'registry'}
          selectedWorkspaceId={automationScopeWorkspaceId}
          selectedAutomation={selectedAutomation}
          collapsed={sidebarCollapsed}
          busy={busy}
          loading={automationsLoading}
          error={automationsError}
          onSelectAutomation={selectAutomation}
          onSetAutomationEnabled={async (automation, enabled) => {
            const updated = await window.beale.updateAutomation({
              runId: automation.runId,
              workspaceId: automation.workspaceId,
              title: automation.title,
              enabled,
              settings: automation.settings
            });
            setAutomations((current) => current.map((candidate) => (
              candidate.runId === updated.runId && candidate.workspaceId === updated.workspaceId ? updated : candidate
            )));
          }}
          onStartNewResearch={() => openNewResearch(true)}
          onStartNewResearchForWorkspace={(workspace) => openNewResearchForWorkspace(workspace, true)}
          onAddWorkspace={() => {
            setAutomationsOpen(false);
            beginWorkspaceCreation();
          }}
          onResizePointerDown={beginSidebarResize}
        />
      ) : (
        <WorkspaceSidebar
          serverSelector={serverSelector}
          busy={busy}
          collapsed={sidebarCollapsed}
          error={error}
          workspaceRegistry={workspaceRegistry}
          workspaceRegistryLoading={startupPhase === 'shell' || startupPhase === 'registry'}
          selectedRunId={reportsOpen || automationsOpen || pluginsOpen ? null : selectedRunId}
          runningTrackSessionId={selectedResearchTrackRunning ? selectedRunId : null}
          runningTrackWorkspaceId={selectedResearchTrackRunning ? snapshot?.workspace.workspaceId : null}
          workspaceCreationActive={workspaceDraft !== null}
          newResearchActive={newResearchOpen}
          automationsActive={automationsOpen}
          pluginsActive={pluginsOpen}
          snapshot={snapshot}
          onAddWorkspace={beginWorkspaceCreation}
          onImportWorkspace={importWorkspace}
          onOpenWorkspace={openWorkspaceFromSidebar}
          onOpenResearchSession={openResearchSessionFromSidebar}
          onArchiveSession={archiveResearchSession}
          onResizePointerDown={beginSidebarResize}
          onStartNewResearch={startNewResearch}
          onOpenQuickChat={openQuickChat}
          onStartNewResearchForWorkspace={startNewResearchForWorkspace}
        />
      )}

      <main className="workbench" data-session-heat={sessionHeat}>
        {settingsOpen ? (
          <SettingsView
            section={settingsSection}
            appearanceTheme={appearanceTheme}
            researchProfiles={researchProfiles}
            researchProfilesLoading={researchProfilesLoading}
            researchProfile={snapshot?.researchProfile ?? null}
            tracesEnabled={tracesEnabled}
            profilingEnabled={profilingState?.enabled ?? false}
            suggestionPreferences={suggestionPreferences}
            dangerModeEnabled={permissionSettings.dangerModeEnabled}
            defaultShellSafetyMode={permissionSettings.defaultShellSafetyMode}
            guardianEnabled={permissionSettings.guardianEnabled}
            onChangeGuardianEnabled={setGuardianEnabled}
            openAiOAuthResult={openAiOAuthResult}
            openAiStatus={openAiStatus ?? snapshot?.openAi ?? null}
            researchProviderOAuthResults={researchProviderOAuthResults}
            researchProviderStatuses={researchProviderStatuses}
            researchProviderModelCatalog={researchProviderModelCatalog}
            providerSettings={providerSettings}
            providerStatusesLoaded={researchProviderStatusesLoaded}
            ticketingSettings={ticketingSettings}
            ticketingTargets={ticketingTargets}
            ticketingLoading={ticketingLoading}
            ticketingError={ticketingError}
            agentPluginState={agentPluginState}
            agentPluginsLoading={agentPluginsLoading}
            agentPluginsBusy={agentPluginsBusy}
            agentPluginsError={agentPluginsError}
            sessionHeatPreferences={sessionHeatPreferences}
            archivedSessions={workspaceRegistry?.archivedResearchSessions ?? []}
            archivedQuickChats={closedQuickChats}
            archiveWorkspaces={workspaceRegistry?.workspaces ?? []}
            archiveLoading={archiveLoading}
            busy={busy}
            onChangeAppearanceTheme={setAppearanceTheme}
            onChangeTracesEnabled={changeTracesEnabled}
            onChangeProfilingEnabled={(enabled) => void setProfilingEnabled(enabled)}
            onChangeSuggestionPreference={setSuggestionPreference}
            onChangeDangerModeEnabled={setDangerModeEnabled}
            onChangeDefaultShellSafetyMode={setDefaultShellSafetyMode}
            onRefreshOpenAi={refreshOpenAiProvider}
            onStartOpenAiOAuth={startOpenAiOAuth}
            onStartResearchProviderOAuth={startResearchProviderOAuth}
            onForgetProviderSubscription={forgetProviderSubscription}
            onRemoveProvider={removeProvider}
            onConfigureProviderApiKey={configureProviderApiKey}
            onRemoveProviderApiKey={removeProviderApiKey}
            onSaveCustomProvider={saveCustomProvider}
            onRemoveCustomProvider={removeCustomProvider}
            onSetDefaultProviderId={setDefaultProviderId}
            onSetProviderModelDefaults={setProviderModelDefaults}
            onSetProviderOptionalModelEnabled={setProviderOptionalModelEnabled}
            onSetProviderContextSize={setProviderContextSize}
            onSetProviderCyberPolicyRiskAcknowledged={setProviderCyberPolicyRiskAcknowledged}
            onSetProviderPreferredAuthenticationMethod={setProviderPreferredAuthenticationMethod}
            onSetAgentPluginEnabled={setAgentPluginEnabled}
            onSetTicketingProvider={changeTicketingProvider}
            onSetTicketingHumanInTheLoop={changeTicketingHumanInTheLoop}
            onConfigureTicketingCredential={configureTicketingCredential}
            onRemoveTicketingCredential={removeTicketingCredential}
            onRefreshTicketingTargets={loadTicketingTargets}
            onSetTicketingTarget={changeTicketingTarget}
            onSetSessionHeatPreference={setSessionHeatPreference}
            onSetSessionHeatPalettePreference={setSessionHeatPalettePreference}
            onRestoreResearchSession={restoreResearchSession}
            onResumeQuickChat={resumeQuickChat}
          />
        ) : fleetOpen ? (
          <div className="settings-workspace">
            <section className="settings-view settings-main-view" aria-label="Fleet">
              <FleetSettingsView
                state={fleetConfiguration}
                workspaces={workspaceRegistry?.workspaces ?? []}
                selectedWorkspaceId={fleetScopeWorkspaceId}
                onScopeChange={setFleetScopeWorkspaceId}
                loadError={fleetConfigurationError}
                remoteAccessSettings={appServerRemoteAccessSettings}
                remoteAccessBusy={appServerRemoteAccessBusy}
                selection={fleetSelection}
                onSelect={setFleetSelection}
                onStateChange={applyFleetConfiguration}
                onRefresh={refreshFleetConfiguration}
                onDetectRemoteAccess={detectAppServerRemoteAccess}
                onSaveRemoteAccess={changeAppServerRemoteAccess}
              />
            </section>
          </div>
        ) : (
          <div className="workspace-page">
            {selectedFleetServerId !== 'local' && !pluginsOpen ? (
              <RemoteFleetWorkspace
                key={selectedFleetServerId}
                serverId={selectedFleetServerId} serverName={selectedFleetServer?.name ?? 'Remote'}
                catalog={remoteCatalog} workspaceId={remoteWorkspaceId} sessionId={remoteSessionId}
                newResearchOpen={remoteResearchOpen} trackMode={remoteTrackMode} automations={automationsOpen} error={remoteCatalogError}
                onStartTrack={() => { setRemoteSessionId(null); setRemoteTrackMode(true); setRemoteResearchOpen(true); }}
                onCancelResearch={() => { setRemoteResearchOpen(false); setRemoteTrackMode(false); }}
                onTrackStarted={(track) => { setRemoteResearchOpen(false); setRemoteTrackMode(false);
                  navigateToResearchTrack(track, selectedFleetServerId);
                  void window.beale.getFleetRemoteCatalog(selectedFleetServerId).then(setRemoteCatalog).catch((caught: unknown) => setRemoteCatalogError(errorMessage(caught))); }}
                onTrackResumed={(track) => navigateToResearchTrack(track, selectedFleetServerId)}
                guardianEnabled={permissionSettings.guardianEnabled}
                selectedSubagentPath={remoteSelectedSubagentPath} onBackToSubagents={() => setRemoteSelectedSubagentPath(null)}
                onSurfaceFocusChange={onRemoteSurfaceFocusChange}
                onWorkspaceViewChange={setRemoteWorkspaceViewName}
                onLaunched={(workspaceId, sessionId) => {
                  setRemoteWorkspaceId(workspaceId);
                  setRemoteSessionId(sessionId);
                  setRemoteResearchOpen(false);
                  setRemoteTrackMode(false);
                  void window.beale.getFleetRemoteCatalog(selectedFleetServerId).then(setRemoteCatalog).catch((caught: unknown) => setRemoteCatalogError(errorMessage(caught)));
                }}
              />
            ) : workspaceDraft ? (
              <WorkspaceCreationView
                busy={busy}
                form={workspaceDraft}
                progress={workspaceOnboardingProgress}
                submissionError={error}
                onCancel={closeWorkspaceOnboarding}
                onChange={setWorkspaceDraft}
                onLookupHackerOne={lookupHackerOneScope}
                onLookupBugcrowd={lookupBugcrowdScope}
                onResearchKit={applyOnboardingResearchKit}
                onSubmit={submitWorkspaceOnboarding}
                onViewChange={setWorkspaceDashboardViewName}
              />
            ) : pluginsOpen ? (
              <PluginManagerWorkspace
                key={`${pluginScopeWorkspaceId ?? 'all'}:${selectedPluginId ?? 'none'}`}
                state={agentPluginState}
                workspaces={workspaceRegistry?.workspaces ?? []}
                selectedWorkspaceId={pluginScopeWorkspaceId}
                selectedPluginId={selectedPluginId}
                loading={agentPluginsLoading}
                busy={agentPluginsBusy}
                error={agentPluginsError}
                onRemove={removeAgentPlugin}
                onScopeChange={(workspaceId) => { setPluginScopeWorkspaceId(workspaceId); setSelectedPluginId(null); }}
                onSetEnabled={setVisibleAgentPluginEnabled}
              />
            ) : automationsOpen ? (
              <AutomationsWorkspace
                automations={automations}
                workspaces={workspaceRegistry?.workspaces ?? []}
                selectedWorkspaceId={automationScopeWorkspaceId}
                selectedAutomation={selectedAutomation}
                detail={renderedRunDetail}
                sessionSetupPending={sessionSetupPending}
                activeScope={snapshot?.activeScope ?? null}
                providerModelDefaults={providerSettings?.modelDefaults}
                providerModelCatalog={enabledResearchProviderModelCatalog}
                shellApproval={inlineApproval}
                shellApprovalBusy={Boolean(inlineApproval && (busy || shellApprovalDecisionInFlight === inlineApproval.id))}
                responseSuggestionsEnabled={suggestionPreferences.responseSuggestionsEnabled}
                busy={busy}
                loading={automationsLoading}
                error={automationsError}
                onScopeChange={(workspaceId) => {
                  setAutomationScopeWorkspaceId(workspaceId);
                  selectAutomation(null);
                }}
                onSelectAutomation={selectAutomation}
                onShellApprovalDecision={decideInlineShellApproval}
                onSessionAction={handleSessionAction}
                onSteerInstruction={handleSteerInstruction}
              />
            ) : reportsOpen ? (
              selectedReport ? (
                <ReportSessionWorkspace
                  report={selectedReport}
                  submissionPacketPath={
                    reportSubmissionPacketPath?.workspaceId === selectedReport.workspaceId
                      && reportSubmissionPacketPath.reportId === selectedReport.id
                      && reportSubmissionPacketPath.artifactId === selectedReport.submissionPacket?.artifactId
                      ? reportSubmissionPacketPath.path
                      : null
                  }
                  document={selectedReportDocument}
                  loading={reportLoading}
                  error={reportError}
                  onReportChange={submitReportChange}
                  onBackToReports={() => {
                    setReportsOpen(false);
                    setReportSessionRunId(null);
                    clearRunDetail();
                    setSelectedRunId(null);
                    backToReports();
                    void loadSnapshot().catch((caught: unknown) => setError(errorMessage(caught)));
                  }}
                  onReportMarkdownChange={async (content) => {
                    try {
                      const updated = await window.beale.updateReportContent({
                        workspaceId: selectedReport.workspaceId,
                        reportId: selectedReport.id,
                        expectedRevision: selectedReport.revision,
                        content
                      });
                      setSelectedReportOverride(updated);
                      setSelectedReportDocument({ reportId: updated.id, content });
                    } catch (caught: unknown) {
                      const message = errorMessage(caught);
                      throw new Error(message);
                    }
                  }}
                  onStatusChange={async (triageStatus) => {
                    try {
                      const updated = await window.beale.updateReportTriageStatus({
                        workspaceId: selectedReport.workspaceId,
                        reportId: selectedReport.id,
                        expectedRevision: selectedReport.revision,
                        triageStatus
                      });
                      setSelectedReportOverride(updated);
                    } catch (caught: unknown) {
                      throw new Error(errorMessage(caught));
                    }
                  }}
                  onChooseSubmissionPacket={async () => {
                    try {
                      const updated = await window.beale.chooseReportSubmissionPacket({
                        workspaceId: selectedReport.workspaceId,
                        reportId: selectedReport.id
                      });
                      if (!updated) return;
                      setSelectedReportOverride(updated);
                    } catch (caught: unknown) {
                      const message = errorMessage(caught);
                      setReportError(message);
                      throw new Error(message);
                    }
                  }}
                  onRevealSubmissionPacket={async () => {
                    try {
                      await window.beale.openReportSubmissionPacket({
                        workspaceId: selectedReport.workspaceId,
                        reportId: selectedReport.id
                      });
                    } catch (caught: unknown) {
                      throw new Error(errorMessage(caught));
                    }
                  }}
                  onChooseRecording={async () => {
                    try {
                      const updated = await window.beale.chooseReportRecording({
                        workspaceId: selectedReport.workspaceId,
                        reportId: selectedReport.id
                      });
                      if (!updated) return;
                      setSelectedReportOverride(updated);
                    } catch (caught: unknown) {
                      const message = errorMessage(caught);
                      setReportError(message);
                      throw new Error(message);
                    }
                  }}
                />
              ) : null
            ) : snapshot ? <MainSessionWorkspace
              workspaceId={snapshot.workspace.workspaceId}
              detail={renderedRunDetail}
              sessionSetupPending={sessionSetupPending || trackSessionAwaitingCreation}
              events={mainSessionTraceEvents}
              onLoadOlderHistory={loadOlderHistory}
              allEvents={activeTraceEvents}
              providerModelCatalog={enabledResearchProviderModelCatalog}
              providerModelDefaults={providerSettings?.modelDefaults}
              appServerMemory={selectedRunId && !newResearchOpen ? null : snapshot?.appServerMemory ?? null}
              activeScope={snapshot?.activeScope ?? null}
              repositoryCheckouts={snapshot?.repositoryCheckouts}
              workspaceRules={selectedRunId && !newResearchOpen ? [] : snapshot?.workspaceRules ?? []}
              researchProfile={selectedRunId && !newResearchOpen ? renderedRunDetail?.researchProfile?.profile ?? null : snapshot?.researchProfile.profile ?? null}
              researchKitId={snapshot.workspace.researchKitId}
              researchSubjectName={selectedRunId && !newResearchOpen ? '' : snapshot?.researchSubject.name ?? ''}
              sessionHeatPreferences={sessionHeatPreferences}
              responseSuggestionsEnabled={suggestionPreferences.responseSuggestionsEnabled}
              workspacePath={selectedRunId && !newResearchOpen ? '' : snapshot?.workspace.workspacePath ?? ''}
              workspaceDirectories={selectedRunId && !newResearchOpen ? [] : snapshot?.workspace.workspaceDirectories}
              workspaceMemoryBackend={snapshot.workspace.memoryBackend ?? 'app-server'}
              workspaceName={snapshot?.activeScope.workspaceName ?? 'Workspace'}
              viewState={newResearchOpen ? 'new-research' : selectedRunId ? 'session' : 'workspace'}
              newResearchContent={newResearchContent}
              runs={selectedRunId ? [] : workspaceDashboardRuns}
              selectedRunId={selectedRunId}
              researchDetailsOpen={rightSidenavExpanded && headerResearchControlsAvailable}
              selectedRunbookId={newResearchOpen ? null : selectedRunbookId}
              selectedRunbook={newResearchOpen ? null : selectedRunbook}
              selectedRunbookDocument={newResearchOpen ? null : selectedRunbookDocument}
              runbookLoading={runbookLoading}
              runbookError={runbookError}
              selectedReportId={newResearchOpen ? null : selectedReportId}
              selectedReport={newResearchOpen ? null : selectedReport}
              selectedReportDocument={newResearchOpen ? null : selectedReportDocument}
              reportLoading={reportLoading}
              reportError={reportError}
              selectedSubagentPath={newResearchOpen ? null : selectedSubagentPath}
              searchHighlightQuery=""
              shellApproval={inlineApproval}
              shellApprovalBusy={Boolean(inlineApproval && (busy || shellApprovalDecisionInFlight === inlineApproval.id))}
              busy={busy}
              connectedDeviceCaptureEnabled={windowControlPlatform === 'darwin'}
              onMarkHistoryDuplicate={markHistoryDuplicate}
              onUndoHistoryDuplicate={undoHistoryDuplicate}
              onTransitionClaim={transitionClaimOnBoard}
              onAddWorkspaceResource={addWorkspaceResource}
              onChangeWorkspaceResource={changeWorkspaceResource}
              onCloneWorkspaceRepository={cloneWorkspaceRepository}
              onRefreshWorkspaceResearchKit={refreshWorkspaceResearchKit}
              onAddWorkspaceRule={addWorkspaceRule}
              onSaveWorkspaceConfiguration={saveWorkspaceConfiguration}
              onChangeWorkspaceDirectories={changeWorkspaceDirectories}
              onChangeWorkspaceMemoryBackend={changeWorkspaceMemoryBackend}
              onChangeWorkspaceResearchSubject={changeWorkspaceResearchSubject}
              onRemoveWorkspace={removeActiveWorkspace}
              onOpenSession={openWorkspaceDashboardSession}
              onStartTrack={openStartTrack}
              onTrackResumed={(track) => navigateToResearchTrack(track, null)}
              initialWorkspaceView={workspaceDashboardWorkspaceIdRef.current === snapshot.workspace.workspaceId
                ? workspaceDashboardInitialView : 'board'}
              onWorkspaceViewChange={setWorkspaceDashboardViewName}
              onWorkspaceViewSelectionChange={setWorkspaceDashboardInitialView}
              onOpenWorkspaceReport={openReportSession}
              onResearchDetailsOpenChange={changeResearchDetailsOpen}
              onOpenAppServerRunbook={openAppServerRunbook}
              onRunAppServerRunbook={runAppServerRunbook}
              onBackToRunbooks={backToRunbooks}
              onOpenAppServerReport={openAppServerReport}
              onBackToReports={backToReports}
              onBackToSubagents={backToSubagents}
              onSelectSubagent={selectSubagent}
              onShellApprovalDecision={decideInlineShellApproval}
              onSessionAction={handleSessionAction}
              onSteerInstruction={handleSteerInstruction}
            /> : <WorkspaceStartupView onAddWorkspace={beginWorkspaceCreation} />}
          </div>
        )}
      </main>
      <QuickChatDock
        chats={selectedFleetServerId === 'local' ? displayedQuickChats : []}
        providerModelCatalog={quickChatInitialModelSelection ? enabledResearchProviderModelCatalog : []}
        initialModelSelection={quickChatInitialModelSelection ?? undefined}
        onClose={closeQuickChat}
        onRunStarted={registerQuickChatRun}
        reportShellApproval={inlineApproval}
        reportShellApprovalBusy={Boolean(inlineApproval && (busy || shellApprovalDecisionInFlight === inlineApproval.id))}
        reportBusy={busy}
        reportResponseSuggestionsEnabled={suggestionPreferences.responseSuggestionsEnabled}
        onReportInitialInstruction={startReportTurn}
        onReportSessionAction={applyReportSessionAction}
        onReportShellApprovalDecision={(decision) => {
          if (inlineApproval) handleShellApprovalDecision(inlineApproval, decision);
        }}
      />
      {selectedFleetServerId !== 'local' ? <RemoteQuickChatDock
        key={selectedFleetServerId}
        serverId={selectedFleetServerId}
        catalog={remoteCatalog}
        chats={remoteQuickChats.filter((chat) => chat.serverId === selectedFleetServerId)}
        onClose={(id) => setRemoteQuickChats((current) => current.filter((chat) => chat.id !== id))}
        onStarted={(id, workspaceId, sessionId) => setRemoteQuickChats((current) => current.map((chat) => chat.id === id ? { ...chat, workspaceId, sessionId } : chat))}
      /> : null}
      <NotificationStack
        notifications={snapshot?.notifications ?? []}
        alerts={workspaceAlerts}
        onOpen={openNotification}
        onDismiss={dismissNotification}
        onOpenAlert={openWorkspaceAlert}
        onDismissAlert={dismissWorkspaceAlert}
      />
      <AppModals
        activeNotification={activeNotification}
        busy={busy}
        profilingOpen={profilingOpen}
        profilingState={profilingState}
        lastProfilingReport={lastProfilingReport}
        missingDirectoryWorkspace={missingDirectoryWorkspace}
        onCloseNotification={() => setActiveNotification(null)}
        onCloseProfiling={closeProfiling}
        onFlushProfilingReport={flushProfilingReport}
        onSetProfilingEnabled={setProfilingEnabled}
        onCloseMissingDirectory={() => setMissingDirectoryWorkspace(null)}
        onRemoveMissingDirectory={removeMissingDirectoryWorkspace}
        onSteerNotification={(notification, instruction) => {
          void runAction(() => window.beale.steerRun({ type: 'steer', runId: notification.runId, instruction }));
          setActiveNotification(null);
        }}
      />
      {activeManualShellApproval ? (
        <ShellApprovalModal
          approval={activeManualShellApproval}
          busy={busy || shellApprovalDecisionInFlight === activeManualShellApproval.id}
          onDecision={(decision) => handleShellApprovalDecision(activeManualShellApproval, decision)}
        />
      ) : null}
    </div>
  );
}

export function appServerRunbookDocumentUpdateKey(document: AppServerRunbookDocument): string {
  const latestRun = document.latestRun;
  const cells = document.cells.map((cell) => {
    const latestCellRun = cell.latestRun;
    const outputs = cell.outputs.map((output) => `${output.kind}:${output.streamName ?? ''}:${output.mimeType ?? ''}:${output.text.length}`).join(',');
    return `${cell.id}:${cell.source.length}:${cell.executionCount ?? ''}:${latestCellRun?.runId ?? ''}:${latestCellRun?.status ?? ''}:${latestCellRun?.durationMs ?? ''}:${outputs}`;
  }).join('|');
  return `${document.runbookId}:${document.revision}:${latestRun?.runId ?? ''}:${latestRun?.status ?? ''}:${latestRun?.durationMs ?? ''}:${cells}`;
}

function isEndedResearchRunStatus(status: RunStatus): boolean {
  return status === 'blocked' || status === 'completed' || status === 'failed' || status === 'stopped';
}

function shellApprovalWorkspacePath(approval: ApprovalRecord): string {
  const workspacePath = approval.requestedAction.workspacePath;
  if (typeof workspacePath !== 'string' || !workspacePath.trim()) {
    throw new Error('Shell approval is missing its originating workspace.');
  }
  return workspacePath;
}
