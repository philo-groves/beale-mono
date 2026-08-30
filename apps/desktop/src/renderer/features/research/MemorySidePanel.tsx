import { memo, useEffect, useMemo, useReducer, useRef, useState } from 'react';
import type { CSSProperties, JSX, ReactNode } from 'react';
import { ArrowLeft, BadgeCheck, BookOpen, Bot, ChevronDown, ChevronRight, Database, FileText, Lightbulb, LoaderCircle, Plus, Search, X } from 'lucide-react';
import type {
  HoneycrispFindingSummary,
  HoneycrispMemoryEdgeSummary,
  HoneycrispMemoryNodeSummary,
  HoneycrispMemorySummary,
  HoneycrispReportDocument,
  HoneycrispReportSummary,
  HoneycrispRunbookDocument,
  HoneycrispRunbookSummary,
  RunbookExecutionSelection,
  RunbookProofTargetSelection,
  ResearchProfile,
  ResearchProfileMemoryStatus,
  ResearchProfileMemoryType,
  ResearchProviderModelCatalog,
  RunDetail,
  RunStatus,
  TraceEventRecord
} from '@shared/types';
import { MainSideScrollRegion } from '../../app/MainSideScrollRegion';
import { FloatingTextPicker } from '../../app/FloatingTextPicker';
import { ProviderIcon } from '../../app/ProviderIcon';
import { ModelAuthors } from '../../app/ModelAuthors';
import { useDevRenderProbe } from '../../devInstrumentation';
import { formatCompactTimeSince, formatSessionDateTime, researchModelDisplayName, stateClass, traceLabel } from '../../lib/formatting';
import { campaignClaimIsActive } from '../../view-models/campaignClaims';
import { activeMemoryCount, filterMemoryCatalogNodes, groupMemoryRelationships, memoryCatalogGroupPreview, memoryCatalogUpdateKey, memoryTypeGroupsByHeat, memoryTypeSummaryPresentation, sessionMemoryActivitySummary, sessionMemoryCatalogNodes, sessionMemoryCreationCount, sessionMemoryTypeSummaries } from '../../view-models/memoryCatalog';
import type { SessionMemoryTypeSummary } from '../../view-models/memoryCatalog';
import { filterSubagentSummaries, subagentCatalogGroups, subagentChannelLabel, subagentDisplayName, subagentOverviewForEvents, subagentOverviewFromSummaries, subagentOverviewStatusCountSummary, subagentStatusIconKind, subagentStatusLabel, subagentSummaries, traceEventsForSubagent } from '../../view-models/subagents';
import type { SubagentStatus, SubagentSummary } from '../../view-models/subagents';
import { runbookBelongsToSession, runbookDescriptionText, runbookExecutionStatus } from '../../view-models/runbooks';
import { reportCatalogGroups } from '../../view-models/reports';
import { EMPTY_SESSION_HEAT_PREFERENCES } from '../../view-models/sessionHeat';
import type { SessionHeatPreferences } from '../../view-models/sessionHeat';
import { researchProfileFeatureAvailability } from '../../view-models/researchProfileFeatures';
import type { TraceDisplayEvent } from '../../view-models/traceDisplay';
import { CommentaryView } from '../commentary/CommentaryView';
import { SessionUsageSummary, SessionUsageSummaryLoading } from '../momentum/SessionUsageStatus';
import { SessionDurationMetric, SessionDurationMetricLoading } from '../sessions/SessionMetrics';
import { MemoryTypeIcon, MemoryTypeLabel, memoryTypeClassName, memoryTypeDefinition, memoryTypeLabel, memoryTypeStyle } from './MemoryTypeLabel';
import { memoryStatusPolarity } from './MemoryStatusDot';
import { RunbookView } from './RunbookView';
import { ReportView } from './ReportView';
import { renderInlineCodeText } from '../traces/traceMarkup';

const EMPTY_SUBAGENT_OVERVIEW = { count: 0, activeCount: 0, completedCount: 0 };
const EMPTY_MEMORY_NODES: HoneycrispMemoryNodeSummary[] = [];
const EMPTY_CAMPAIGN_CLAIMS: readonly HoneycrispFindingSummary[] = [];
const EMPTY_RUNBOOKS: readonly HoneycrispRunbookSummary[] = [];

export type MemoryLevelFilter = 'session' | 'workspace' | 'subject';
export const DEFAULT_MEMORY_LEVEL_FILTER: MemoryLevelFilter = 'session';
export const DEFAULT_WORKSPACE_MEMORY_LEVEL_FILTER: MemoryLevelFilter = 'workspace';
export type CampaignClaimFilter = 'all' | 'none' | 'leads' | 'findings';
export type CampaignRunbookFilter = 'all' | 'none';
export const NO_CAMPAIGN_MEMORIES_FILTER = '__no_memories__';
export type CampaignEntryTarget = 'top' | 'leads' | 'findings' | 'memories' | 'runbooks';
export type RunbookScopeFilter = 'session' | 'workspace';
export const DEFAULT_RUNBOOK_SCOPE_FILTER: RunbookScopeFilter = 'session';
export const DEFAULT_WORKSPACE_RUNBOOK_SCOPE_FILTER: RunbookScopeFilter = 'workspace';
export type ResearchViewSpace = 'session' | 'workspace';
export type ResearchSideView = 'memory' | 'reports' | 'subagents';

export interface ResearchSideNavigationState {
  openViews: ResearchSideView[];
  activeView: ResearchSideView | null;
}

export type ResearchSideNavigationAction =
  | { type: 'open'; view: ResearchSideView }
  | { type: 'activate'; view: ResearchSideView }
  | { type: 'close'; view: ResearchSideView }
  | { type: 'restrict'; views: readonly ResearchSideView[] }
  | { type: 'reset' };

export const RESEARCH_SIDE_VIEWS: readonly ResearchSideView[] = ['memory', 'reports', 'subagents'];

export function memoryLevelFiltersForViewSpace(viewSpace: ResearchViewSpace): MemoryLevelFilter[] {
  return viewSpace === 'workspace' ? ['workspace', 'subject'] : ['session', 'workspace', 'subject'];
}

export function runbookScopeFiltersForViewSpace(viewSpace: ResearchViewSpace): RunbookScopeFilter[] {
  return viewSpace === 'workspace' ? ['workspace'] : ['session', 'workspace'];
}

export function campaignCatalogSectionVisibility(
  claimFilter: CampaignClaimFilter,
  memoryType: string
): { memories: boolean; leads: boolean; findings: boolean } {
  return {
    memories: memoryType !== NO_CAMPAIGN_MEMORIES_FILTER,
    leads: claimFilter === 'all' || claimFilter === 'leads',
    findings: claimFilter === 'all' || claimFilter === 'findings'
  };
}

export function campaignEntryFilterSelection(
  target: CampaignEntryTarget
): { claimFilter: CampaignClaimFilter; memoryType: string; runbookFilter: CampaignRunbookFilter } | null {
  if (target === 'leads') {
    return { claimFilter: 'leads', memoryType: NO_CAMPAIGN_MEMORIES_FILTER, runbookFilter: 'none' };
  }
  if (target === 'findings') {
    return { claimFilter: 'findings', memoryType: NO_CAMPAIGN_MEMORIES_FILTER, runbookFilter: 'none' };
  }
  if (target === 'memories') {
    return { claimFilter: 'none', memoryType: 'all', runbookFilter: 'none' };
  }
  if (target === 'runbooks') {
    return { claimFilter: 'none', memoryType: NO_CAMPAIGN_MEMORIES_FILTER, runbookFilter: 'all' };
  }
  return null;
}

export function campaignEntryScrollTargetSelector(target: CampaignEntryTarget): string | undefined {
  if (target === 'memories') return '[data-memory-type]';
  if (target === 'leads') return '[data-campaign-projection="leads"]';
  if (target === 'findings') return '[data-campaign-projection="findings"]';
  if (target === 'runbooks') return '[data-campaign-runbooks]';
  return undefined;
}

export function campaignEntryScope(
  viewSpace: ResearchViewSpace,
  target: CampaignEntryTarget
): MemoryLevelFilter | null {
  if (target === 'top') return null;
  return viewSpace === 'workspace' ? DEFAULT_WORKSPACE_MEMORY_LEVEL_FILTER : DEFAULT_MEMORY_LEVEL_FILTER;
}

export function researchViewSpaceLabel(viewSpace: ResearchViewSpace): 'Session' | 'Workspace' {
  return viewSpace === 'workspace' ? 'Workspace' : 'Session';
}

const CLOSED_RESEARCH_SIDE_NAVIGATION: ResearchSideNavigationState = {
  openViews: [],
  activeView: null
};

export function researchSideNavigationReducer(
  state: ResearchSideNavigationState,
  action: ResearchSideNavigationAction
): ResearchSideNavigationState {
  if (action.type === 'reset') return CLOSED_RESEARCH_SIDE_NAVIGATION;
  if (action.type === 'restrict') return restrictResearchSideNavigation(state, action.views);
  if (action.type === 'open') {
    return {
      openViews: state.openViews.includes(action.view) ? state.openViews : [...state.openViews, action.view],
      activeView: action.view
    };
  }
  if (action.type === 'activate') {
    return state.openViews.includes(action.view) ? { ...state, activeView: action.view } : state;
  }

  const closingIndex = state.openViews.indexOf(action.view);
  if (closingIndex < 0) return state;
  const openViews = state.openViews.filter((view) => view !== action.view);
  if (state.activeView !== action.view) return { openViews, activeView: state.activeView };
  return {
    openViews,
    activeView: openViews[Math.min(closingIndex, openViews.length - 1)] ?? null
  };
}

export function availableResearchSideViews(
  openViews: readonly ResearchSideView[],
  enabledViews: readonly ResearchSideView[] = RESEARCH_SIDE_VIEWS
): ResearchSideView[] {
  return enabledViews.filter((view) => !openViews.includes(view));
}

export function researchSideViewsForProfile(
  profile: ResearchProfile | null | undefined
): ResearchSideView[] {
  const features = researchProfileFeatureAvailability(profile);
  return RESEARCH_SIDE_VIEWS.filter((view) => (
    view === 'memory'
      ? features.memory || features.runbooks
      : view === 'reports'
        ? features.reports
        : features.collaboration
  ));
}

export function restrictResearchSideNavigation(
  state: ResearchSideNavigationState,
  enabledViews: readonly ResearchSideView[]
): ResearchSideNavigationState {
  const openViews = state.openViews.filter((view) => enabledViews.includes(view));
  return {
    openViews,
    activeView: state.activeView && openViews.includes(state.activeView)
      ? state.activeView
      : openViews.at(-1) ?? null
  };
}

export function isLastOpenResearchSideView(
  openViews: readonly ResearchSideView[],
  view: ResearchSideView
): boolean {
  return openViews.length === 1 && openViews[0] === view;
}

function initialResearchSideNavigation(
  selectedSubagentPath: string | null,
  selectedRunbookId: string | null,
  selectedReportId: string | null,
  enabledViews: readonly ResearchSideView[]
): ResearchSideNavigationState {
  return researchSideNavigationForSelectedDetail(
    CLOSED_RESEARCH_SIDE_NAVIGATION,
    selectedSubagentPath,
    selectedRunbookId,
    selectedReportId,
    enabledViews
  );
}

export function researchSideNavigationForSelectedDetail(
  state: ResearchSideNavigationState,
  selectedSubagentPath: string | null,
  selectedRunbookId: string | null,
  selectedReportId: string | null,
  enabledViews: readonly ResearchSideView[]
): ResearchSideNavigationState {
  if (selectedSubagentPath && enabledViews.includes('subagents')) {
    return researchSideNavigationReducer(state, { type: 'open', view: 'subagents' });
  }
  if (selectedRunbookId && enabledViews.includes('memory')) {
    return researchSideNavigationReducer(state, { type: 'open', view: 'memory' });
  }
  if (selectedReportId && enabledViews.includes('reports')) {
    return researchSideNavigationReducer(state, { type: 'open', view: 'reports' });
  }
  return state;
}

export const ResearchSidePanel = memo(function ResearchSidePanel({
  detail,
  events,
  memory,
  researchProfile = null,
  sessionHeatPreferences = EMPTY_SESSION_HEAT_PREFERENCES,
  runId,
  runStatus,
  providerModelCatalog,
  selectedRunbook,
  selectedRunbookDocument,
  runbookLoading,
  runbookError,
  connectedDeviceOs = null,
  selectedReport = null,
  selectedReportDocument = null,
  reportLoading = false,
  reportError = null,
  selectedSubagentPath,
  selectedRunbookId,
  selectedReportId = null,
  selectedClaimId,
  selectedMemoryNodeId,
  searchHighlightQuery,
  onOpenRunbook,
  onRunbookExecute,
  onOpenReport = () => undefined,
  onOpenClaim,
  onSelectSubagent,
  onBackToRunbooks,
  onBackToReports = () => undefined,
  onBackToClaim,
  onBackToMemory,
  onBackToSubagents,
  expanded,
  onExpandedChange,
  viewSpace = 'session'
}: {
  detail: RunDetail | null;
  events: TraceDisplayEvent[];
  memory: HoneycrispMemorySummary | null;
  researchProfile?: ResearchProfile | null;
  sessionHeatPreferences?: SessionHeatPreferences;
  runId: string;
  runStatus: RunStatus | null;
  providerModelCatalog: ResearchProviderModelCatalog[];
  selectedRunbook: HoneycrispRunbookSummary | null;
  selectedRunbookDocument: HoneycrispRunbookDocument | null;
  runbookLoading: boolean;
  runbookError: string | null;
  connectedDeviceOs?: string | null;
  selectedReport?: HoneycrispReportSummary | null;
  selectedReportDocument?: HoneycrispReportDocument | null;
  reportLoading?: boolean;
  reportError?: string | null;
  selectedSubagentPath: string | null;
  selectedRunbookId: string | null;
  selectedReportId?: string | null;
  selectedClaimId?: string | null;
  selectedMemoryNodeId?: string | null;
  searchHighlightQuery: string;
  onOpenRunbook: (runbookId: string) => void;
  onRunbookExecute?: (runbookId: string, selection: RunbookExecutionSelection, target: RunbookProofTargetSelection) => Promise<void>;
  onOpenReport?: (reportId: string) => void;
  onOpenClaim?: (claimId: string) => void;
  onSelectSubagent: (path: string) => void;
  onBackToRunbooks: () => void;
  onBackToReports?: () => void;
  onBackToClaim?: () => void;
  onBackToMemory?: () => void;
  onBackToSubagents: () => void;
  expanded?: boolean;
  onExpandedChange?: (expanded: boolean) => void;
  viewSpace?: ResearchViewSpace;
}): JSX.Element {
  const featureAvailability = researchProfileFeatureAvailability(researchProfile);
  const subagentsAvailable = featureAvailability.collaboration && viewSpace === 'session';
  const enabledViews = researchSideViewsForProfile(researchProfile)
    .filter((view) => viewSpace === 'session' || view !== 'subagents');
  const enabledViewsKey = enabledViews.join(':');
  const [navigation, dispatchNavigation] = useReducer(
    researchSideNavigationReducer,
    initialResearchSideNavigation(selectedSubagentPath, selectedRunbookId, selectedReportId, enabledViews)
  );
  const runIdRef = useRef(runId);
  const [query, setQuery] = useState('');
  const [reportQuery, setReportQuery] = useState('');
  const [subagentQuery, setSubagentQuery] = useState('');
  const [catalogNowMs, setCatalogNowMs] = useState(() => Date.now());
  const [scope, setScope] = useState<MemoryLevelFilter>(
    viewSpace === 'workspace' ? DEFAULT_WORKSPACE_MEMORY_LEVEL_FILTER : DEFAULT_MEMORY_LEVEL_FILTER
  );
  const [reportScope, setReportScope] = useState<RunbookScopeFilter>(
    viewSpace === 'workspace' ? DEFAULT_WORKSPACE_RUNBOOK_SCOPE_FILTER : DEFAULT_RUNBOOK_SCOPE_FILTER
  );
  const [claimFilter, setClaimFilter] = useState<CampaignClaimFilter>('all');
  const [type, setType] = useState('all');
  const [runbookFilter, setRunbookFilter] = useState<CampaignRunbookFilter>('all');
  const [expandedMemoryGroups, setExpandedMemoryGroups] = useState<ReadonlySet<string>>(new Set());
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedClaimIdState, setSelectedClaimIdState] = useState<string | null>(null);
  const [campaignEntryRequest, setCampaignEntryRequest] = useState<{ key: number; target: CampaignEntryTarget }>({ key: 0, target: 'top' });
  useEffect(() => {
    const timer = window.setInterval(() => setCatalogNowMs(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);
  const visibleSelectedNodeId = selectedMemoryNodeId === undefined ? selectedNodeId : selectedMemoryNodeId;
  const visibleSelectedClaimId = selectedClaimId === undefined ? selectedClaimIdState : selectedClaimId;
  const restrictedNavigation = restrictResearchSideNavigation(navigation, enabledViews);
  const visibleNavigation = researchSideNavigationForSelectedDetail(
    restrictedNavigation,
    selectedSubagentPath,
    selectedRunbookId,
    selectedReportId,
    enabledViews
  );
  const detailsOpen = expanded ?? visibleNavigation.openViews.length > 0;
  const activeView = (visibleSelectedNodeId || visibleSelectedClaimId) && enabledViews.includes('memory') ? 'memory' : visibleNavigation.activeView;
  const nodes = memory?.nodes ?? [];
  const runbooks = memory?.runbooks ?? [];
  const reports = memory?.reports ?? [];
  const leads = memory?.leads ?? [];
  const findings = memory?.findings ?? [];
  const memoryProfile = researchProfile?.memory;
  const memoryTypes = memoryProfile?.types ?? [];
  const memoryStatuses = memoryProfile?.statuses ?? [];
  const memoryLabel = 'Memory';
  const memoriesLabel = 'Memories';
  const campaignLabel = 'Campaign';
  const runbookLabel = 'Runbooks';
  const reportLabel = 'Reports';
  const viewSpaceLabel = researchViewSpaceLabel(viewSpace);
  const summaryEvents: readonly TraceEventRecord[] = events.length > 0 ? events : detail?.traceEvents ?? [];
  const sessionMemoryNodes = useMemo(
    () => sessionMemoryCatalogNodes(nodes, runId),
    [nodes, runId]
  );
  const sessionMemories = useMemo(
    () => activeMemoryCount(sessionMemoryNodes, memoryProfile?.statuses),
    [memoryProfile?.statuses, sessionMemoryNodes]
  );
  const sessionMemoryCreations = useMemo(
    () => sessionMemoryCreationCount(
      sessionMemoryNodes,
      detail?.run.startedAt ?? detail?.run.createdAt,
      detail?.run.endedAt
    ),
    [detail?.run.createdAt, detail?.run.endedAt, detail?.run.startedAt, sessionMemoryNodes]
  );
  const sessionMemoryActivity = useMemo(
    () => sessionMemoryActivitySummary(summaryEvents, sessionMemoryCreations, detail?.activityCounts),
    [detail?.activityCounts, sessionMemoryCreations, summaryEvents]
  );
  const sessionMemoryTypes = useMemo(
    () => memoryTypeSummaryPresentation(
      sessionMemoryTypeSummaries(sessionMemoryNodes, memoryProfile),
      memoryProfile,
      researchProfile?.id,
      sessionHeatPreferences.heatOverrides
    ),
    [memoryProfile, researchProfile?.id, sessionHeatPreferences.heatOverrides, sessionMemoryNodes]
  );
  const workspaceId = memory?.contextWorkspaceId ?? null;
  const subjectId = memory?.contextSubjectId ?? null;
  const sessionRunbookItems = useMemo(
    () => runbooks.filter((runbook) => runbookBelongsToSession(runbook, runId)),
    [runbooks, runId]
  );
  const sessionRunbooks = sessionRunbookItems.length;
  const sessionRunbookMetrics = useMemo(() => summarizeRunbookMetrics(sessionRunbookItems), [sessionRunbookItems]);
  const sessionReports = useMemo(() => reports.filter((report) => report.sessionId === runId), [reports, runId]);
  const sessionReportRevisions = useMemo(() => sessionReports.reduce((count, report) => count + report.revision, 0), [sessionReports]);
  const sessionFindings = useMemo(
    () => filterCampaignClaims(findings, { query: '', scope: 'session', sessionId: runId, workspaceId, subjectId }),
    [findings, runId, subjectId, workspaceId]
  );
  const sessionLeads = useMemo(
    () => filterCampaignClaims(leads, { query: '', scope: 'session', sessionId: runId, workspaceId, subjectId }),
    [leads, runId, subjectId, workspaceId]
  );
  const activeSessionFindings = useMemo(
    () => sessionFindings.filter(campaignClaimIsActive),
    [sessionFindings]
  );
  const activeSessionLeads = useMemo(
    () => sessionLeads.filter(campaignClaimIsActive),
    [sessionLeads]
  );
  const workspaceMemoryNodes = useMemo(
    () => filterMemoryCatalogNodes(nodes, {
      query: '',
      scope: 'workspace',
      sessionId: runId,
      workspaceId,
      subjectId,
      type: 'all'
    }),
    [nodes, runId, subjectId, workspaceId]
  );
  const workspaceMemories = useMemo(
    () => activeMemoryCount(workspaceMemoryNodes, memoryProfile?.statuses),
    [memoryProfile?.statuses, workspaceMemoryNodes]
  );
  const workspaceMemoryTypes = useMemo(
    () => memoryTypeSummaryPresentation(
      sessionMemoryTypeSummaries(workspaceMemoryNodes, memoryProfile),
      memoryProfile,
      researchProfile?.id,
      sessionHeatPreferences.heatOverrides
    ),
    [memoryProfile, researchProfile?.id, sessionHeatPreferences.heatOverrides, workspaceMemoryNodes]
  );
  const workspaceRunbooks = useMemo(
    () => runbooks.filter((runbook) => workspaceId !== null && runbook.workspaceId === workspaceId),
    [runbooks, workspaceId]
  );
  const workspaceRunbookMetrics = useMemo(() => summarizeRunbookMetrics(workspaceRunbooks), [workspaceRunbooks]);
  const workspaceReports = useMemo(
    () => reports.filter((report) => workspaceId !== null && report.workspaceId === workspaceId),
    [reports, workspaceId]
  );
  const workspaceFindings = useMemo(
    () => filterCampaignClaims(findings, { query: '', scope: 'workspace', sessionId: runId, workspaceId, subjectId }),
    [findings, runId, subjectId, workspaceId]
  );
  const workspaceLeads = useMemo(
    () => filterCampaignClaims(leads, { query: '', scope: 'workspace', sessionId: runId, workspaceId, subjectId }),
    [leads, runId, subjectId, workspaceId]
  );
  const workspaceReportRevisions = useMemo(
    () => workspaceReports.reduce((count, report) => count + report.revision, 0),
    [workspaceReports]
  );
  const workspaceClaimCount = workspaceLeads.length + workspaceFindings.length;
  const workspaceClaimSummary = claimProjectionSummaryText(workspaceLeads.length, workspaceFindings.length);
  const sessionLeadRatingSummary = claimRatingSummaryText(activeSessionLeads);
  const sessionFindingRatingSummary = claimRatingSummaryText(activeSessionFindings);
  const workspaceRunbookSummaryMetrics = runbookSummaryMetricsText(workspaceRunbookMetrics);
  const sessionRunbookSummaryMetrics = runbookSummaryMetricsText(sessionRunbookMetrics);
  const needsFullSubagents = subagentsAvailable && (detailsOpen || selectedSubagentPath !== null);
  const subagents = useMemo(
    () => needsFullSubagents
      ? subagentSummaries(summaryEvents, runStatus, 'commentary', detail?.subagentPreviews)
      : [],
    [detail?.subagentPreviews, needsFullSubagents, runStatus, summaryEvents]
  );
  const subagentOverview = useMemo(
    () => {
      if (!subagentsAvailable) return EMPTY_SUBAGENT_OVERVIEW;
      return needsFullSubagents ? subagentOverviewFromSummaries(subagents) : subagentOverviewForEvents(summaryEvents, runStatus);
    },
    [needsFullSubagents, runStatus, subagents, subagentsAvailable, summaryEvents]
  );
  const filteredSubagents = useMemo(
    () => needsFullSubagents ? filterSubagentSummaries(subagents, subagentQuery) : [],
    [needsFullSubagents, subagentQuery, subagents]
  );
  const groupedSubagents = useMemo(() => subagentCatalogGroups(filteredSubagents), [filteredSubagents]);
  const visibleSelectedSubagentPath = subagentsAvailable ? selectedSubagentPath : null;
  const visibleSelectedRunbookId = featureAvailability.runbooks ? selectedRunbookId : null;
  const visibleSelectedRunbook = visibleSelectedRunbookId ? selectedRunbook : null;
  const visibleSelectedReportId = featureAvailability.reports ? selectedReportId : null;
  const visibleSelectedReport = visibleSelectedReportId ? selectedReport : null;
  const selectedSubagent = visibleSelectedSubagentPath
    ? subagents.find((subagent) => subagent.path === visibleSelectedSubagentPath) ?? null
    : null;
  const selectedSubagentName = subagentDisplayName(visibleSelectedSubagentPath
    ? selectedSubagent?.name
      ?? visibleSelectedSubagentPath.split('/').filter(Boolean).at(-1)
      ?? visibleSelectedSubagentPath
    : '');
  const selectedRunbookName = visibleSelectedRunbook?.title
    ?? runbooks.find((runbook) => runbook.id === visibleSelectedRunbookId)?.title
    ?? 'Loading runbook';
  const selectedReportName = visibleSelectedReport?.title
    ?? reports.find((report) => report.id === visibleSelectedReportId)?.title
    ?? 'Loading report';
  const selectedSubagentEvents = useMemo(
    () => traceEventsForSubagent(events, visibleSelectedSubagentPath),
    [events, visibleSelectedSubagentPath]
  );
  const subagentStatusCounts = useMemo(() => subagentOverviewStatusCountSummary(subagentOverview), [subagentOverview]);
  const nodeTypes = useMemo(() => orderedCatalogMemoryTypes(nodes, memoryTypes), [memoryTypes, nodes]);
  const filteredNodes = useMemo(
    () => filterMemoryCatalogNodes(nodes, { query, scope, sessionId: runId, workspaceId, subjectId, type }),
    [nodes, query, runId, scope, subjectId, type, workspaceId]
  );
  const filteredFindings = useMemo(
    () => filterCampaignClaims(findings, { query, scope, sessionId: runId, workspaceId, subjectId }),
    [findings, query, runId, scope, subjectId, workspaceId]
  );
  const filteredLeads = useMemo(
    () => filterCampaignClaims(leads, { query, scope, sessionId: runId, workspaceId, subjectId }),
    [leads, query, runId, scope, subjectId, workspaceId]
  );
  const campaignSectionVisibility = campaignCatalogSectionVisibility(claimFilter, type);
  const visibleFilteredNodes = featureAvailability.memory && campaignSectionVisibility.memories ? filteredNodes : EMPTY_MEMORY_NODES;
  const visibleFilteredFindings = featureAvailability.memory && campaignSectionVisibility.findings ? filteredFindings : EMPTY_CAMPAIGN_CLAIMS;
  const visibleFilteredLeads = featureAvailability.memory && campaignSectionVisibility.leads ? filteredLeads : EMPTY_CAMPAIGN_CLAIMS;
  const memoryTypeGroups = useMemo(
    () => memoryTypeGroupsByHeat(
      [...visibleFilteredNodes].sort((left, right) => right.updatedAt.localeCompare(left.updatedAt) || left.id.localeCompare(right.id)),
      memoryTypes,
      researchProfile?.id,
      sessionHeatPreferences.heatOverrides
    ),
    [memoryTypes, researchProfile?.id, sessionHeatPreferences.heatOverrides, visibleFilteredNodes]
  );
  const filteredRunbooks = useMemo(
    () => filterCampaignRunbooks(runbooks, { query, scope, sessionId: runId, workspaceId, subjectId }),
    [query, runbooks, runId, scope, subjectId, workspaceId]
  );
  const visibleFilteredRunbooks = featureAvailability.runbooks && runbookFilter === 'all' ? filteredRunbooks : EMPTY_RUNBOOKS;
  const filteredReports = useMemo(
    () => filterReportCatalog(reports, reportScope, runId, workspaceId, reportQuery),
    [reportQuery, reports, reportScope, runId, workspaceId]
  );
  const groupedReports = useMemo(() => reportCatalogGroups(filteredReports), [filteredReports]);
  const nodeById = useMemo(() => new Map(nodes.map((node) => [node.id, node])), [nodes]);
  const selectedNode = featureAvailability.memory && visibleSelectedNodeId ? nodeById.get(visibleSelectedNodeId) ?? null : null;
  const claimById = useMemo(
    () => new Map([...findings, ...leads].map((claim) => [claim.id, claim])),
    [findings, leads]
  );
  const selectedClaim = featureAvailability.memory && visibleSelectedClaimId ? claimById.get(visibleSelectedClaimId) ?? null : null;
  const relationshipsByNodeId = useMemo(() => groupMemoryRelationships(memory?.edges ?? []), [memory?.edges]);
  const updateKey = [
    ...visibleFilteredFindings.map((claim) => `${claim.id}:${claim.updatedAt}:${claim.revision}`),
    ...visibleFilteredLeads.map((claim) => `${claim.id}:${claim.updatedAt}:${claim.revision}`),
    memoryCatalogUpdateKey(visibleFilteredNodes),
    ...visibleFilteredRunbooks.map((runbook) => `${runbook.id}:${runbook.updatedAt}:${runbook.revision}`)
  ].join('|');
  const reportUpdateKey = filteredReports.map((report) => `${report.id}:${report.updatedAt}`).join('|');
  const campaignRecordCount = (featureAvailability.memory ? nodes.length + findings.length + leads.length : 0)
    + (featureAvailability.runbooks ? runbooks.length : 0);
  const visibleCampaignRecordCount = visibleFilteredNodes.length + visibleFilteredFindings.length
    + visibleFilteredLeads.length + visibleFilteredRunbooks.length;
  const hasSessionMetadata = Boolean(detail);
  const hasSessionResources = (featureAvailability.runbooks && sessionRunbooks > 0)
    || (featureAvailability.reports && sessionReports.length > 0)
    || (featureAvailability.collaboration && subagentOverview.count > 0);
  const hasSessionMemories = featureAvailability.memory;

  useEffect(() => {
    if (runIdRef.current === runId) return;
    runIdRef.current = runId;
    dispatchNavigation({ type: 'reset' });
    setExpandedMemoryGroups(new Set());
    setSelectedNodeId(null);
    setSelectedClaimIdState(null);
    setScope(viewSpace === 'workspace' ? DEFAULT_WORKSPACE_MEMORY_LEVEL_FILTER : DEFAULT_MEMORY_LEVEL_FILTER);
    setReportScope(viewSpace === 'workspace' ? DEFAULT_WORKSPACE_RUNBOOK_SCOPE_FILTER : DEFAULT_RUNBOOK_SCOPE_FILTER);
  }, [runId, viewSpace]);

  useEffect(() => {
    dispatchNavigation({ type: 'restrict', views: enabledViews });
    if (!featureAvailability.memory) {
      setSelectedNodeId(null);
      setSelectedClaimIdState(null);
    }
  }, [enabledViewsKey, featureAvailability.memory]);

  useEffect(() => {
    if (selectedSubagentPath && subagentsAvailable) {
      dispatchNavigation({ type: 'open', view: 'subagents' });
      return;
    }
    if (selectedRunbookId && featureAvailability.runbooks) {
      dispatchNavigation({ type: 'open', view: 'memory' });
      return;
    }
    if (selectedReportId && featureAvailability.reports) {
      dispatchNavigation({ type: 'open', view: 'reports' });
    }
  }, [
    featureAvailability.reports,
    featureAvailability.runbooks,
    selectedReportId,
    selectedRunbookId,
    selectedSubagentPath,
    subagentsAvailable
  ]);

  useEffect(() => {
    if (!featureAvailability.runbooks && selectedRunbookId) onBackToRunbooks();
    if (!featureAvailability.reports && selectedReportId) onBackToReports();
    if (!subagentsAvailable && selectedSubagentPath) onBackToSubagents();
  }, [
    featureAvailability.runbooks,
    featureAvailability.reports,
    onBackToRunbooks,
    onBackToReports,
    onBackToSubagents,
    selectedRunbookId,
    selectedReportId,
    selectedSubagentPath,
    subagentsAvailable
  ]);

  useEffect(() => {
    if (selectedNodeId && !nodeById.has(selectedNodeId)) setSelectedNodeId(null);
  }, [nodeById, selectedNodeId]);

  useEffect(() => {
    if (selectedClaimId === undefined && selectedClaimIdState && !claimById.has(selectedClaimIdState)) setSelectedClaimIdState(null);
  }, [claimById, selectedClaimId, selectedClaimIdState]);

  useDevRenderProbe('research.memory', () => ({
    loaded: Boolean(memory),
    nodes: nodes.length,
    visibleNodes: filteredNodes.length,
    scope,
    type
  }));

  const openDetails = (view: ResearchSideView, campaignEntryTarget: CampaignEntryTarget = 'top'): void => {
    if (!enabledViews.includes(view)) return;
    if (view === 'memory') {
      const entrySelection = campaignEntryFilterSelection(campaignEntryTarget);
      if (entrySelection) {
        setClaimFilter(entrySelection.claimFilter);
        setType(entrySelection.memoryType);
        setRunbookFilter(entrySelection.runbookFilter);
      }
      const entryScope = campaignEntryScope(viewSpace, campaignEntryTarget);
      if (entryScope) setScope(entryScope);
      setCampaignEntryRequest((current) => ({ key: current.key + 1, target: campaignEntryTarget }));
    }
    if (view !== 'memory') {
      setSelectedNodeId(null);
      setSelectedClaimIdState(null);
    }
    dispatchNavigation({ type: 'open', view });
    onExpandedChange?.(true);
  };

  const activateDetails = (view: ResearchSideView): void => {
    if (!enabledViews.includes(view)) return;
    if (view !== 'memory') {
      setSelectedNodeId(null);
      setSelectedClaimIdState(null);
    }
    dispatchNavigation({ type: 'activate', view });
  };

  const closeDetails = (view: ResearchSideView): void => {
    if (view === 'memory') {
      setSelectedNodeId(null);
      setSelectedClaimIdState(null);
    }
    const closingLastView = isLastOpenResearchSideView(visibleNavigation.openViews, view);
    dispatchNavigation({ type: 'close', view });
    if (closingLastView) onExpandedChange?.(false);
  };

  const openClaimDetail = (claimId: string): void => {
    setSelectedNodeId(null);
    if (selectedClaimId === undefined) setSelectedClaimIdState(claimId);
    else onOpenClaim?.(claimId);
  };

  const closeClaimDetail = (): void => {
    if (selectedClaimId === undefined) setSelectedClaimIdState(null);
    else onBackToClaim?.();
  };

  const closeSubagentDetail = (): void => {
    dispatchNavigation({ type: 'open', view: 'subagents' });
    onBackToSubagents();
  };

  if (!detailsOpen) {
    if (viewSpace === 'workspace') {
      return (
        <aside className="main-session-side session-summary-panel workspace-summary-panel" aria-label="Workspace summary">
          <section className="session-summary-card">
            <header className="session-summary-heading">
              <h2 className="session-summary-title">Workspace</h2>
            </header>
            <section className="session-summary-items session-summary-resources" aria-label="Workspace resources">
              {featureAvailability.runbooks && workspaceRunbooks.length > 0 ? (
                <button type="button" className="session-summary-item" onClick={() => openDetails('memory', 'runbooks')}>
                  <BookOpen size={15} aria-hidden="true" />
                  <span>{workspaceRunbooks.length} {workspaceRunbooks.length === 1 ? 'Runbook' : runbookLabel}</span>
                  {workspaceRunbookSummaryMetrics ? <span className="session-summary-meta">{workspaceRunbookSummaryMetrics}</span> : null}
                  <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
                </button>
              ) : null}
              {featureAvailability.reports && workspaceReports.length > 0 ? (
                <button type="button" className="session-summary-item" onClick={() => openDetails('reports')}>
                  <FileText size={15} aria-hidden="true" />
                  <span>{workspaceReports.length} {workspaceReports.length === 1 ? 'Report' : 'Reports'}</span>
                  <span className="session-summary-meta">{workspaceReportRevisions} Updates</span>
                  <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
                </button>
              ) : null}
              {featureAvailability.memory && workspaceClaimCount > 0 ? (
                <button type="button" className="session-summary-item" onClick={() => openDetails('memory')}>
                  <FileText size={15} aria-hidden="true" />
                  <span>{workspaceClaimCount} {workspaceClaimCount === 1 ? 'Claim' : 'Claims'}</span>
                  <span className="session-summary-meta">{workspaceClaimSummary}</span>
                  <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
                </button>
              ) : null}
              {featureAvailability.memory ? (
                <>
                  <button type="button" className="session-summary-item" onClick={() => openDetails('memory', 'memories')}>
                    <Database size={15} aria-hidden="true" />
                    <span>{workspaceMemories} {workspaceMemories === 1 ? memoryLabel : memoriesLabel}</span>
                    <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
                  </button>
                  <MemoryTypeSummaryRows
                    key={`workspace:${workspaceId ?? runId}`}
                    summaries={workspaceMemoryTypes.summaries}
                    defaultVisibleCount={0}
                  />
                </>
              ) : null}
            </section>
          </section>
        </aside>
      );
    }
    if (!detail) {
      return (
        <aside className="main-session-side session-summary-panel" aria-label="Session summary" aria-busy="true">
          <section className="session-summary-card">
            <header className="session-summary-heading">
              <h2 className="session-summary-title">Session</h2>
              <SessionDurationMetricLoading className="session-summary-duration" />
            </header>
            <section className="session-summary-section session-summary-metadata" aria-label="Session metadata">
              <SessionUsageSummaryLoading />
            </section>
            {hasSessionMemories ? <hr className="session-summary-divider" /> : null}
            {hasSessionMemories ? (
              <section className="session-summary-items session-summary-resources" aria-label="Session resources">
                <div aria-label="Loading session memories" className="session-summary-item session-summary-loading-item">
                  <Database aria-hidden="true" size={15} />
                  <span aria-hidden="true" className="session-summary-loading-line" />
                </div>
              </section>
            ) : null}
          </section>
        </aside>
      );
    }
    return (
      <aside className="main-session-side session-summary-panel" aria-label="Session summary">
        <section className="session-summary-card">
          <header className="session-summary-heading">
            <h2 className="session-summary-title">Session</h2>
            {detail ? <SessionDurationMetric detail={detail} className="session-summary-duration" /> : null}
          </header>
          <section className="session-summary-section session-summary-metadata" aria-label="Session metadata">
            {detail ? <SessionUsageSummary detail={detail} /> : null}
          </section>
          {hasSessionMetadata && (hasSessionResources || hasSessionMemories) ? <hr className="session-summary-divider" /> : null}
          <section className="session-summary-items session-summary-resources" aria-label="Session resources">
            {featureAvailability.collaboration && subagentOverview.count > 0 ? (
              <button type="button" className="session-summary-item" onClick={() => openDetails('subagents')}>
                <Bot size={15} aria-hidden="true" />
                <span>{subagentOverview.count} {subagentOverview.count === 1 ? 'Subagent' : 'Subagents'}</span>
                {subagentStatusCounts ? <span className="session-summary-meta">{subagentStatusCounts}</span> : null}
                <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
              </button>
            ) : null}
            {featureAvailability.runbooks && sessionRunbooks > 0 ? (
              <button type="button" className="session-summary-item" onClick={() => openDetails('memory', 'runbooks')}>
                <BookOpen size={15} aria-hidden="true" />
                <span>{sessionRunbooks} {sessionRunbooks === 1 ? 'Runbook' : runbookLabel}</span>
                {sessionRunbookSummaryMetrics ? <span className="session-summary-meta">{sessionRunbookSummaryMetrics}</span> : null}
                <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
              </button>
            ) : null}
            {featureAvailability.reports && sessionReports.length > 0 ? (
              <button type="button" className="session-summary-item" onClick={() => openDetails('reports')}>
                <FileText size={15} aria-hidden="true" />
                <span>{sessionReports.length} {sessionReports.length === 1 ? 'Report' : 'Reports'}</span>
                <span className="session-summary-meta">{sessionReportRevisions} Updates</span>
                <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
              </button>
            ) : null}
            {featureAvailability.memory && activeSessionLeads.length > 0 ? (
              <button type="button" className="session-summary-item" onClick={() => openDetails('memory', 'leads')}>
                <CampaignClaimProjectionIcon projection="lead" />
                <span>{activeSessionLeads.length} {activeSessionLeads.length === 1 ? 'Lead' : 'Leads'}</span>
                {sessionLeadRatingSummary ? <span className="session-summary-meta">{sessionLeadRatingSummary}</span> : null}
                <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
              </button>
            ) : null}
            {featureAvailability.memory && activeSessionFindings.length > 0 ? (
              <button type="button" className="session-summary-item" onClick={() => openDetails('memory', 'findings')}>
                <CampaignClaimProjectionIcon projection="finding" />
                <span>{activeSessionFindings.length} {activeSessionFindings.length === 1 ? 'Finding' : 'Findings'}</span>
                {sessionFindingRatingSummary ? <span className="session-summary-meta">{sessionFindingRatingSummary}</span> : null}
                <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
              </button>
            ) : null}
            {featureAvailability.memory ? (
              <>
                <button type="button" className="session-summary-item" onClick={() => openDetails('memory', 'memories')}>
                  <Database size={15} aria-hidden="true" />
                  <span>{sessionMemories} {sessionMemories === 1 ? memoryLabel : memoriesLabel}</span>
                  {sessionMemoryActivity ? <span className="session-summary-meta">{sessionMemoryActivity}</span> : null}
                  <ChevronRight className="session-summary-chevron" size={15} aria-hidden="true" />
                </button>
                <MemoryTypeSummaryRows
                  key={`session:${runId}`}
                  summaries={sessionMemoryTypes.summaries}
                  defaultVisibleCount={0}
                />
              </>
            ) : null}
          </section>
        </section>
      </aside>
    );
  }

  if (!activeView) {
    return (
      <aside className="main-session-side memory-catalog view-empty" aria-label={`${viewSpaceLabel} details`}>
        <ResearchSideViewChooser viewSpaceLabel={viewSpaceLabel} views={enabledViews} labels={{ memory: campaignLabel, reports: reportLabel }} onOpen={openDetails} />
      </aside>
    );
  }

  return (
    <>
      <aside className={`main-session-side memory-catalog view-${activeView} ${visibleSelectedSubagentPath || visibleSelectedRunbookId || visibleSelectedReportId || selectedNode || selectedClaim ? 'has-nested-view' : ''}`} aria-label={`${viewSpaceLabel} details`}>
        {visibleSelectedSubagentPath ? (
          <ResearchSideNestedHeader
            label="Subagents"
            name={selectedSubagentName}
            primaryLeading={<SubagentStatusIndicator status={selectedSubagent?.status ?? 'unknown'} />}
            secondary={(
              <>
                <SubagentProviderIcon provider={selectedSubagent?.provider ?? null} model={selectedSubagent?.model ?? null} />
                <span>{subagentModelDisplayName(selectedSubagent?.provider ?? null, selectedSubagent?.model ?? null, providerModelCatalog)}</span>
              </>
            )}
            stackedInlineIdentity
            onBack={closeSubagentDetail}
          />
        ) : visibleSelectedRunbookId ? (
          <ResearchSideNestedHeader label={campaignLabel} name={selectedRunbookName} onBack={onBackToRunbooks} />
        ) : visibleSelectedReportId ? (
          <ResearchSideNestedHeader label={reportLabel} name={selectedReportName} onBack={onBackToReports} />
        ) : selectedNode ? (
          <ResearchSideNestedHeader
            label={campaignLabel}
            name={selectedNode.title}
            onBack={() => selectedMemoryNodeId === undefined ? setSelectedNodeId(null) : onBackToMemory?.()}
          />
        ) : selectedClaim ? (
          <ResearchSideNestedHeader
            label={campaignLabel}
            onBack={closeClaimDetail}
          />
        ) : (
          <ResearchSideViewTabs
            activeView={activeView}
            enabledViews={enabledViews}
            labels={{ memory: campaignLabel, reports: reportLabel }}
            openViews={visibleNavigation.openViews}
            viewSpaceLabel={viewSpaceLabel}
            onActivate={activateDetails}
            onClose={closeDetails}
            onOpen={openDetails}
            trailing={activeView === 'memory' ? (
              <FloatingTextPicker
                className="memory-catalog-filter memory-catalog-level-filter research-side-memory-scope"
                value={scope}
                title="Campaign scope filter"
                ariaLabel="Campaign scope filter"
                options={memoryLevelFiltersForViewSpace(viewSpace).map((filter) => ({
                  value: filter,
                  label: filter === 'session'
                    ? researchViewSpaceLabel('session')
                    : filter === 'workspace'
                      ? researchViewSpaceLabel('workspace')
                      : researchProfile?.workspace.subjectNoun ?? 'Subject'
                }))}
                onChange={(value) => setScope(value as MemoryLevelFilter)}
              />
            ) : activeView === 'reports' ? (
              <FloatingTextPicker
                className="memory-catalog-filter memory-catalog-level-filter research-side-runbook-scope"
                value={reportScope}
                title="Report scope filter"
                ariaLabel="Report scope filter"
                options={runbookScopeFiltersForViewSpace(viewSpace).map((filter) => ({
                  value: filter,
                  label: researchViewSpaceLabel(filter)
                }))}
                onChange={(value) => setReportScope(value as RunbookScopeFilter)}
              />
            ) : null}
          />
        )}

        {visibleSelectedSubagentPath ? (
          <div className="research-side-nested-content subagent-chat-content">
            <CommentaryView
              busy={false}
              detail={detail}
              events={selectedSubagentEvents}
              providerModelCatalog={providerModelCatalog}
              selectedRunId={runId}
              showBackToMain
              showBackButton={false}
              scrollScopeKey={visibleSelectedSubagentPath}
              searchHighlightQuery={searchHighlightQuery}
              onBackToMain={closeSubagentDetail}
              onSessionAction={() => undefined}
              onSteerInstruction={() => undefined}
            />
          </div>
        ) : visibleSelectedRunbook ? (
          <div className="research-side-nested-content runbook-detail-content">
            <RunbookView
              connectedDeviceOs={connectedDeviceOs}
              document={selectedRunbookDocument}
              error={runbookError}
              executionAvailable={viewSpace === 'session' && runStatus === 'active' && runbookBelongsToSession(visibleSelectedRunbook, runId)}
              followLatest
              loading={runbookLoading}
              runbook={visibleSelectedRunbook}
              showBackButton={false}
              onBackToMain={onBackToRunbooks}
              onRun={onRunbookExecute ? (selection, target) => onRunbookExecute(visibleSelectedRunbook.id, selection, target) : undefined}
            />
          </div>
        ) : visibleSelectedRunbookId ? (
          <div className="memory-catalog-empty">Loading runbook.</div>
        ) : visibleSelectedReport ? (
          <div className="research-side-nested-content runbook-detail-content">
            <ReportView
              document={selectedReportDocument}
              error={reportError}
              loading={reportLoading}
              report={visibleSelectedReport}
            />
          </div>
        ) : visibleSelectedReportId ? (
          <div className="memory-catalog-empty">Loading report.</div>
        ) : selectedNode ? (
          <MainSideScrollRegion
            className="research-side-nested-content memory-detail-content"
            listClassName="memory-catalog-list memory-detail-scroll"
            updateKey={`${selectedNode.id}:${selectedNode.updatedAt}:${selectedNode.revision}`}
          >
            <MemoryDetailView
              node={selectedNode}
              nodeById={nodeById}
              relationships={relationshipsByNodeId.get(selectedNode.id) ?? []}
              researchProfile={researchProfile}
              sessionHeatPreferences={sessionHeatPreferences}
            />
          </MainSideScrollRegion>
        ) : selectedClaim ? (
          <MainSideScrollRegion
            className="research-side-nested-content memory-detail-content"
            listClassName="memory-catalog-list memory-detail-scroll"
            updateKey={`${selectedClaim.id}:${selectedClaim.updatedAt}:${selectedClaim.revision}`}
          >
            <CampaignClaimDetailView claim={selectedClaim} providerModelCatalog={providerModelCatalog} />
          </MainSideScrollRegion>
        ) : activeView === 'memory' ? (
          <>
            <div className="memory-catalog-controls">
              <div className="memory-catalog-search">
                <Search size={14} aria-hidden="true" />
                <input
                  type="search"
                  value={query}
                  placeholder={`Search ${campaignLabel}`}
                  aria-label={`Search ${campaignLabel.toLocaleLowerCase()}`}
                  onChange={(event) => setQuery(event.target.value)}
                />
                <div className="memory-catalog-inline-filters" aria-label="Campaign filters">
                  {featureAvailability.memory ? (
                    <>
                      <FloatingTextPicker
                        className="memory-catalog-filter memory-catalog-claim-filter"
                        value={claimFilter}
                        title="Claim filter"
                        ariaLabel="Claim filter"
                        options={[
                          { value: 'all', label: 'All Claims' },
                          { value: 'none', label: 'No Claims' },
                          { value: 'leads', label: 'Leads' },
                          { value: 'findings', label: 'Findings' }
                        ]}
                        onChange={(value) => {
                          const nextClaimFilter = value as CampaignClaimFilter;
                          setClaimFilter(nextClaimFilter);
                          if (nextClaimFilter === 'leads' || nextClaimFilter === 'findings') {
                            setType(NO_CAMPAIGN_MEMORIES_FILTER);
                            setRunbookFilter('none');
                          }
                        }}
                      />
                      <FloatingTextPicker
                        className="memory-catalog-filter memory-catalog-type-filter"
                        value={type}
                        title="Memory type filter"
                        ariaLabel="Memory type filter"
                        options={[
                          { value: 'all', label: `All ${memoriesLabel}` },
                          { value: NO_CAMPAIGN_MEMORIES_FILTER, label: `No ${memoriesLabel}` },
                          ...nodeTypes.map((nodeType) => ({
                            value: nodeType.id,
                            label: nodeType.label,
                            group: nodeType.group,
                            className: `memory-type-option ${memoryTypeClassName(nodeType.id, memoryTypes)}`,
                            style: memoryTypeStyle(
                              nodeType.id,
                              memoryTypes,
                              undefined,
                              researchProfile?.id,
                              sessionHeatPreferences
                            )
                          }))
                        ]}
                        onChange={(value) => {
                          setType(value);
                          if (value !== 'all' && value !== NO_CAMPAIGN_MEMORIES_FILTER) {
                            setClaimFilter('none');
                            setRunbookFilter('none');
                          }
                        }}
                      />
                    </>
                  ) : null}
                  {featureAvailability.runbooks ? (
                    <FloatingTextPicker
                      className="memory-catalog-filter memory-catalog-runbook-filter"
                      value={runbookFilter}
                      title="Runbook filter"
                      ariaLabel="Runbook filter"
                      options={[
                        { value: 'all', label: `All ${runbookLabel}` },
                        { value: 'none', label: `No ${runbookLabel}` }
                      ]}
                      onChange={(value) => setRunbookFilter(value as CampaignRunbookFilter)}
                    />
                  ) : null}
                </div>
              </div>
            </div>
            {!memory ? <div className="memory-catalog-empty">Loading {campaignLabel.toLocaleLowerCase()}.</div> : null}
            {memory?.lastError ? <div className="memory-catalog-empty is-error">{memory.lastError}</div> : null}
            {memory && !memory.lastError && campaignRecordCount === 0 ? <div className="memory-catalog-empty">No campaign records yet.</div> : null}
            {memory && campaignRecordCount > 0 && visibleCampaignRecordCount === 0 ? <div className="memory-catalog-empty">No records match these filters.</div> : null}
            {visibleCampaignRecordCount > 0 ? (
              <MainSideScrollRegion
                initialScrollRequestKey={campaignEntryScrollTargetSelector(campaignEntryRequest.target) ? campaignEntryRequest.key : undefined}
                initialScrollTargetSelector={campaignEntryScrollTargetSelector(campaignEntryRequest.target)}
                listClassName="memory-catalog-list memory-type-groups"
                stickToStart
                updateKey={updateKey}
              >
                <CampaignClaimCatalogLists
                  findings={visibleFilteredFindings}
                  leads={visibleFilteredLeads}
                  nowMs={catalogNowMs}
                  providerModelCatalog={providerModelCatalog}
                  selectedClaimId={visibleSelectedClaimId}
                  onOpen={openClaimDetail}
                />
                {memoryTypeGroups.map((memoryTypeGroup) => (
                  <MemoryTypeCatalogSection
                    nowMs={catalogNowMs}
                    expanded={expandedMemoryGroups.has(memoryTypeGroup.type)}
                    key={memoryTypeGroup.type}
                    memoryStatuses={memoryStatuses}
                    memoryTypes={memoryTypes}
                    nodes={memoryTypeGroup.nodes}
                    profileId={researchProfile?.id}
                    providerModelCatalog={providerModelCatalog}
                    sessionHeatPreferences={sessionHeatPreferences}
                    selectedNodeId={visibleSelectedNodeId}
                    type={memoryTypeGroup.type}
                    onExpand={() => setExpandedMemoryGroups((current) => new Set(current).add(memoryTypeGroup.type))}
                    onOpen={(nodeId) => {
                      setSelectedClaimIdState(null);
                      setSelectedNodeId(nodeId);
                    }}
                  />
                ))}
                {visibleFilteredRunbooks.length > 0 ? (
                  <CampaignRunbookCatalogSection
                    nowMs={catalogNowMs}
                    providerModelCatalog={providerModelCatalog}
                    runbooks={visibleFilteredRunbooks}
                    selectedRunbookId={selectedRunbookId}
                    onOpen={onOpenRunbook}
                  />
                ) : null}
              </MainSideScrollRegion>
            ) : null}
          </>
        ) : activeView === 'reports' ? (
          <>
            <CatalogSearch value={reportQuery} placeholder="Find a Report" ariaLabel="Search reports" onChange={setReportQuery} />
            <MainSideScrollRegion listClassName="memory-catalog-list runbook-catalog-list report-catalog-list" stickToStart updateKey={reportUpdateKey}>
              {filteredReports.length === 0 ? (
                <p className="runbook-catalog-empty">
                  {reportQuery.trim() ? 'No reports match this search.' : 'No reports yet.'}
                </p>
              ) : (
                <>
                  {groupedReports.complete.length > 0 ? (
                    <ReportCatalogSection
                      label="Complete"
                      nowMs={catalogNowMs}
                      reports={groupedReports.complete}
                      selectedReportId={selectedReportId}
                      onOpen={onOpenReport}
                    />
                  ) : null}
                  {groupedReports.stale.length > 0 ? (
                    <ReportCatalogSection
                      label="Stale"
                      nowMs={catalogNowMs}
                      reports={groupedReports.stale}
                      selectedReportId={selectedReportId}
                      onOpen={onOpenReport}
                    />
                  ) : null}
                </>
              )}
            </MainSideScrollRegion>
          </>
        ) : (
          <>
            <CatalogSearch value={subagentQuery} placeholder="Find a Subagent" ariaLabel="Search subagents" onChange={setSubagentQuery} />
            <MainSideScrollRegion
              listClassName="subagent-catalog-list"
              stickToStart
              updateKey={filteredSubagents.map((agent) => `${agent.path}:${agent.provider}:${agent.model}:${agent.status}:${agent.createdAt}:${agent.lastActiveAt}:${agent.latestMessage}`).join('|')}
            >
              {filteredSubagents.length === 0 ? (
                <p className="subagent-catalog-empty">
                  {subagentQuery.trim() ? 'No subagents match this search.' : 'No subagents yet.'}
                </p>
              ) : (
                <>
                  {groupedSubagents.active.length > 0 ? (
                    <SubagentCatalogSection
                      agents={groupedSubagents.active}
                      nowMs={catalogNowMs}
                      providerModelCatalog={providerModelCatalog}
                      label="Active"
                      onSelect={onSelectSubagent}
                      selectedPath={visibleSelectedSubagentPath}
                    />
                  ) : null}
                  {groupedSubagents.completed.length > 0 ? (
                    <SubagentCatalogSection
                      agents={groupedSubagents.completed}
                      nowMs={catalogNowMs}
                      providerModelCatalog={providerModelCatalog}
                      label="Completed"
                      onSelect={onSelectSubagent}
                      selectedPath={visibleSelectedSubagentPath}
                    />
                  ) : null}
                </>
              )}
            </MainSideScrollRegion>
          </>
        )}
      </aside>
    </>
  );
});

export function filterCampaignRunbooks(
  runbooks: readonly HoneycrispRunbookSummary[],
  filters: {
    query: string;
    scope: MemoryLevelFilter;
    sessionId: string;
    workspaceId: string | null;
    subjectId: string | null;
  }
): HoneycrispRunbookSummary[] {
  const normalizedQuery = filters.query.trim().toLocaleLowerCase();
  return runbooks.filter((runbook) => {
    const inScope = filters.scope === 'session'
      ? runbookBelongsToSession(runbook, filters.sessionId)
      : filters.scope === 'workspace'
        ? filters.workspaceId !== null && runbook.workspaceId === filters.workspaceId
        : filters.subjectId !== null && runbook.subjectId === filters.subjectId;
    if (!inScope || !normalizedQuery) return inScope;
    return [
      runbook.id,
      runbook.title,
      runbook.purpose,
      runbookExecutionStatus(runbook).label,
      runbook.workspaceId,
      runbook.workspaceName,
      runbook.subjectId ?? '',
      runbook.subjectName ?? '',
      runbook.sessionId ?? '',
      runbook.artifactId
    ].join('\n').toLocaleLowerCase().includes(normalizedQuery);
  }).sort((left, right) => right.updatedAt.localeCompare(left.updatedAt) || left.id.localeCompare(right.id));
}

export function filterCampaignClaims(
  claims: readonly HoneycrispFindingSummary[],
  filters: {
    query: string;
    scope: MemoryLevelFilter;
    sessionId: string;
    workspaceId: string | null;
    subjectId: string | null;
  }
): HoneycrispFindingSummary[] {
  const normalizedQuery = filters.query.trim().toLocaleLowerCase();
  return claims
    .filter((claim) => {
      const inScope = filters.scope === 'session'
        ? claim.originSessionId === filters.sessionId || claim.evidence.some((evidence) => evidence.sessionId === filters.sessionId)
        : filters.scope === 'workspace'
          ? filters.workspaceId !== null && claim.workspaceId === filters.workspaceId
          : filters.subjectId !== null && claim.subjectId === filters.subjectId;
      if (!inScope || !normalizedQuery) return inScope;
      return [
        claim.id,
        claim.title,
        claim.summary,
        claim.impact,
        claim.classification,
        claim.status,
        claim.maturity,
        claim.workflow,
        claim.rating,
        ...claim.evidence.map((evidence) => evidence.summary)
      ].join('\n').toLocaleLowerCase().includes(normalizedQuery);
    })
    .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt) || left.id.localeCompare(right.id));
}

export function filterReportCatalog(
  reports: readonly HoneycrispReportSummary[],
  scope: RunbookScopeFilter,
  sessionId: string,
  workspaceId: string | null,
  query = ''
): HoneycrispReportSummary[] {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  return reports.filter((report) => {
    const inScope = scope === 'session'
      ? report.sessionId === sessionId
      : workspaceId !== null && report.workspaceId === workspaceId;
    if (!inScope || !normalizedQuery) return inScope;
    return [report.id, report.title, report.summary, report.status, report.workspaceName, report.subjectName ?? '']
      .join('\n').toLocaleLowerCase().includes(normalizedQuery);
  });
}

function CatalogSearch({
  value,
  placeholder,
  ariaLabel,
  onChange
}: {
  value: string;
  placeholder: string;
  ariaLabel: string;
  onChange: (value: string) => void;
}): JSX.Element {
  return (
    <div className="memory-catalog-controls">
      <div className="memory-catalog-search search-only">
        <Search size={14} aria-hidden="true" />
        <input
          type="search"
          value={value}
          placeholder={placeholder}
          aria-label={ariaLabel}
          onChange={(event) => onChange(event.target.value)}
        />
      </div>
    </div>
  );
}

function MemoryTypeSummaryRows({ summaries, defaultVisibleCount }: {
  summaries: readonly SessionMemoryTypeSummary[];
  defaultVisibleCount: number;
}): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const visibleSummaries = summaries.slice(0, defaultVisibleCount);
  const hiddenSummaries = summaries.slice(defaultVisibleCount);
  return (
    <div className="session-memory-type-list">
      {visibleSummaries.map((memoryType) => <MemoryTypeSummaryRow memoryType={memoryType} key={memoryType.type} />)}
      {hiddenSummaries.length > 0 ? (
        <>
          <div
            className={`session-memory-type-overflow ${expanded ? 'expanded' : ''}`.trim()}
            aria-hidden={!expanded}
            inert={!expanded}
          >
            <div className="session-memory-type-overflow-inner">
              {hiddenSummaries.map((memoryType) => <MemoryTypeSummaryRow memoryType={memoryType} key={memoryType.type} />)}
            </div>
          </div>
          <button
            type="button"
            className="session-memory-type-toggle"
            aria-expanded={expanded}
            onClick={() => setExpanded((current) => !current)}
          >
            {expanded ? 'Show less' : 'Show more'}
          </button>
        </>
      ) : null}
    </div>
  );
}

function MemoryTypeSummaryRow({ memoryType }: { memoryType: SessionMemoryTypeSummary }): JSX.Element {
  return (
    <div className="session-memory-type-item">
      <span>{memoryType.countLabel}</span>
      {memoryType.statusLabel ? <span className="session-summary-meta">{memoryType.statusLabel}</span> : null}
    </div>
  );
}

export function ResearchSideNestedHeader({
  label,
  leading,
  name,
  primaryLeading,
  secondary,
  stackedInlineIdentity = false,
  onBack
}: {
  label: string;
  leading?: ReactNode;
  name?: string;
  primaryLeading?: ReactNode;
  secondary?: ReactNode;
  stackedInlineIdentity?: boolean;
  onBack: () => void;
}): JSX.Element {
  return (
    <header className={`research-side-nested-header ${stackedInlineIdentity ? 'is-stacked-inline' : ''}`.trim()}>
      <button type="button" onClick={onBack}>
        <ArrowLeft size={15} aria-hidden="true" />
        <span>Back to {label}</span>
      </button>
      {leading}
      {name ? (
        <span className={`research-side-nested-identity ${secondary ? 'has-secondary' : ''}`}>
          <span className="research-side-nested-primary">
            {primaryLeading}
            <span className="research-side-nested-name" title={name}>{name}</span>
            {stackedInlineIdentity && secondary ? <span className="research-side-nested-secondary">{secondary}</span> : null}
          </span>
          {!stackedInlineIdentity && secondary ? <span className="research-side-nested-secondary">{secondary}</span> : null}
        </span>
      ) : null}
    </header>
  );
}

export function SubagentDetailPanel({
  detail,
  error = null,
  events,
  providerModelCatalog,
  runId,
  subagent,
  backLabel = 'Subagents',
  onBack
}: {
  detail: RunDetail | null;
  error?: string | null;
  events: TraceDisplayEvent[];
  providerModelCatalog: ResearchProviderModelCatalog[];
  runId: string;
  subagent: Pick<SubagentSummary, 'path' | 'name' | 'provider' | 'model'> & { status?: SubagentStatus | 'unknown' };
  backLabel?: string;
  onBack: () => void;
}): JSX.Element {
  const name = subagentDisplayName(subagent.name);
  return (
    <aside className="main-session-side memory-catalog view-subagents has-nested-view" aria-label="Subagent details">
      <ResearchSideNestedHeader
        label={backLabel}
        name={name}
        primaryLeading={<SubagentStatusIndicator status={subagent.status ?? 'unknown'} />}
        secondary={(
          <>
            <SubagentProviderIcon provider={subagent.provider} model={subagent.model} />
            <span>{subagentModelDisplayName(subagent.provider, subagent.model, providerModelCatalog)}</span>
          </>
        )}
        stackedInlineIdentity
        onBack={onBack}
      />
      <div className="research-side-nested-content subagent-chat-content">
        {error ? (
          <div className="main-trace-empty" role="status">{error}</div>
        ) : (
          <CommentaryView
            busy={false}
            detail={detail}
            events={events}
            providerModelCatalog={providerModelCatalog}
            selectedRunId={runId}
            showBackToMain
            showBackButton={false}
            scrollScopeKey={subagent.path}
            searchHighlightQuery=""
            onBackToMain={onBack}
            onSessionAction={() => undefined}
            onSteerInstruction={() => undefined}
          />
        )}
      </div>
    </aside>
  );
}

export function ResearchSideViewTabs({
  activeView,
  enabledViews = RESEARCH_SIDE_VIEWS,
  openViews,
  onActivate,
  onClose,
  onOpen,
  labels,
  trailing,
  viewSpaceLabel = 'Session'
}: {
  activeView: ResearchSideView;
  enabledViews?: readonly ResearchSideView[];
  openViews: readonly ResearchSideView[];
  onActivate: (view: ResearchSideView) => void;
  onClose: (view: ResearchSideView) => void;
  onOpen: (view: ResearchSideView) => void;
  labels?: Partial<Record<ResearchSideView, string>>;
  trailing?: ReactNode;
  viewSpaceLabel?: string;
}): JSX.Element {
  const [pickerOpen, setPickerOpen] = useState(false);
  const pickerRef = useRef<HTMLDivElement | null>(null);
  const availableViews = availableResearchSideViews(openViews, enabledViews);

  useEffect(() => {
    if (!pickerOpen) return undefined;
    const closeOnPointerDown = (event: PointerEvent): void => {
      if (!pickerRef.current?.contains(event.target as Node)) setPickerOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') setPickerOpen(false);
    };
    window.addEventListener('pointerdown', closeOnPointerDown);
    window.addEventListener('keydown', closeOnEscape);
    return () => {
      window.removeEventListener('pointerdown', closeOnPointerDown);
      window.removeEventListener('keydown', closeOnEscape);
    };
  }, [pickerOpen]);

  useEffect(() => {
    if (availableViews.length === 0) setPickerOpen(false);
  }, [availableViews.length]);

  return (
    <header className="research-side-view-header">
      <div className="research-side-view-tabs research-side-view-tabs-scrollable" role="tablist" aria-label={`Open ${viewSpaceLabel.toLocaleLowerCase()} detail views`}>
        {openViews.map((view) => (
          <div className={`research-side-view-tab ${activeView === view ? 'active' : ''}`} key={view}>
            <button
              type="button"
              className="research-side-view-tab-activate"
              role="tab"
              aria-selected={activeView === view}
              onClick={() => onActivate(view)}
            >
              {researchSideViewIcon(view, 15)}
              <span>{researchSideViewLabel(view, labels)}</span>
            </button>
            <button
              type="button"
              className="research-side-view-tab-close"
              aria-label={`Close ${researchSideViewLabel(view, labels)}`}
              title={`Close ${researchSideViewLabel(view, labels)}`}
              onClick={() => onClose(view)}
            >
              <X size={13} aria-hidden="true" />
            </button>
          </div>
        ))}
      </div>
      {availableViews.length > 0 ? (
        <div className={`research-side-view-picker ${pickerOpen ? 'open' : ''}`} ref={pickerRef}>
          <button
            type="button"
            className="research-side-view-picker-trigger"
            aria-label={`Add ${viewSpaceLabel.toLocaleLowerCase()} detail view`}
            aria-haspopup="menu"
            aria-expanded={pickerOpen}
            title={`Add ${viewSpaceLabel.toLocaleLowerCase()} detail view`}
            onClick={() => setPickerOpen((current) => !current)}
          >
            <Plus size={16} aria-hidden="true" />
          </button>
          {pickerOpen ? (
            <div className="research-side-view-picker-menu" role="menu">
              {availableViews.map((view) => (
                <button
                  type="button"
                  role="menuitem"
                  key={view}
                  onClick={() => {
                    onOpen(view);
                    setPickerOpen(false);
                  }}
                >
                  {researchSideViewIcon(view, 15)}
                  <span>{researchSideViewLabel(view, labels)}</span>
                </button>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}
      {trailing ? <div className="research-side-view-trailing">{trailing}</div> : null}
    </header>
  );
}

export function ResearchSideViewChooser({
  labels,
  onOpen,
  views = RESEARCH_SIDE_VIEWS,
  viewSpaceLabel = 'Session'
}: {
  labels?: Partial<Record<ResearchSideView, string>>;
  onOpen: (view: ResearchSideView) => void;
  views?: readonly ResearchSideView[];
  viewSpaceLabel?: string;
}): JSX.Element {
  return (
    <nav className="research-side-view-chooser" aria-label={`Choose a ${viewSpaceLabel.toLocaleLowerCase()} detail view`}>
      {views.map((view) => (
        <button type="button" key={view} onClick={() => onOpen(view)}>
          {researchSideViewIcon(view, 16)}
          <span>{researchSideViewLabel(view, labels)}</span>
        </button>
      ))}
    </nav>
  );
}

function researchSideViewLabel(
  view: ResearchSideView,
  labels?: Partial<Record<ResearchSideView, string>>
): string {
  return labels?.[view] ?? (view === 'memory'
    ? 'Campaign'
    : view === 'reports'
      ? 'Reports'
      : 'Subagents');
}

interface CatalogMemoryTypeOption {
  id: string;
  label: string;
  group?: string;
}

export function orderedCatalogMemoryTypes(
  nodes: readonly HoneycrispMemoryNodeSummary[],
  definitions: readonly ResearchProfileMemoryType[]
): CatalogMemoryTypeOption[] {
  return [...new Set(nodes.map((node) => node.type))]
    .map((id) => ({ id, definition: memoryTypeDefinition(id, definitions) }))
    .sort((left, right) => {
      if (left.definition && right.definition) {
        const group = (left.definition.group ?? '').localeCompare(right.definition.group ?? '');
        return left.definition.order - right.definition.order
          || group
          || left.definition.name.localeCompare(right.definition.name)
          || left.id.localeCompare(right.id);
      }
      if (left.definition) return -1;
      if (right.definition) return 1;
      return left.id.localeCompare(right.id);
    })
    .map(({ id, definition }) => ({
      id,
      label: memoryTypeLabel(id, definitions),
      ...(definition?.group ? { group: definition.group } : {})
    }));
}

export function pluralizePresentationLabel(label: string): string {
  const trimmed = label.trim();
  if (!trimmed || /s$/iu.test(trimmed)) return trimmed;
  if (/[^aeiou]y$/iu.test(trimmed)) return `${trimmed.slice(0, -1)}ies`;
  return `${trimmed}s`;
}

function singularizePresentationLabel(label: string): string {
  const trimmed = label.trim();
  if (/ies$/iu.test(trimmed)) return `${trimmed.slice(0, -3)}y`;
  if (/s$/iu.test(trimmed)) return trimmed.slice(0, -1);
  return trimmed;
}

function unknownProfileValueLabel(kind: string, id: string): string {
  const normalized = id.trim().replace(/[_-]+/gu, ' ') || 'unlabeled';
  return `Unknown ${kind} (${normalized})`;
}

function researchSideViewIcon(view: ResearchSideView, size: number): JSX.Element {
  if (view === 'memory') return <Database size={size} aria-hidden="true" />;
  if (view === 'reports') return <FileText size={size} aria-hidden="true" />;
  return <Bot size={size} aria-hidden="true" />;
}

function CatalogTime({ value, nowMs }: { value: string; nowMs: number }): JSX.Element {
  return (
    <time className="catalog-time-since" dateTime={value} title={formatSessionDateTime(value)}>
      {formatCompactTimeSince(value, nowMs)}
    </time>
  );
}

export function RunbookCatalogItem({
  runbook,
  compactTime = false,
  nowMs = Date.now(),
  providerModelCatalog = [],
  selected,
  onOpen
}: {
  runbook: HoneycrispRunbookSummary;
  compactTime?: boolean;
  nowMs?: number;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  selected: boolean;
  onOpen: () => void;
}): JSX.Element {
  const metrics = summarizeRunbookMetrics([runbook]);
  const executionStatus = runbookExecutionStatus(runbook);
  return (
    <button
      type="button"
      className={`runbook-catalog-item runbook-execution-${executionStatus.id} ${compactTime ? 'is-compact' : ''} ${selected ? 'selected' : ''}`}
      aria-pressed={selected}
      onClick={onOpen}
    >
      <span className="runbook-catalog-heading">
        <span className="runbook-catalog-primary">
          <BookOpen
            className="runbook-catalog-icon"
            size={16}
            aria-label={`Latest run: ${executionStatus.label}`}
          />
          <span className="runbook-catalog-name">{runbook.title}</span>
        </span>
        <span className="runbook-catalog-heading-trailing">
          {compactTime
            ? <CatalogTime value={runbook.updatedAt} nowMs={nowMs} />
            : <time dateTime={runbook.updatedAt} title={formatSessionDateTime(runbook.updatedAt)}>{formatSessionDateTime(runbook.updatedAt)}</time>}
        </span>
      </span>
      {runbook.purpose ? <span className="runbook-catalog-purpose">{runbookDescriptionText(runbook.purpose)}</span> : null}
      {compactTime ? (
        <span className="runbook-catalog-context">
          <span className="runbook-catalog-metrics">
            {runbookMetricLabels(metrics).map((label) => <span key={label}>{label}</span>)}
          </span>
          <MemoryCatalogAuthors authors={runbook.authors} providerModelCatalog={providerModelCatalog} />
        </span>
      ) : <span className="runbook-catalog-metrics">{runbookMetricsText(metrics)}</span>}
    </button>
  );
}

interface RunbookMetricsPresentation {
  contentRevisions: number;
  completedRuns: number;
  executedCells: number;
  latestStatus: 'running' | 'succeeded' | 'failed' | 'blocked' | null;
}

function summarizeRunbookMetrics(runbooks: readonly HoneycrispRunbookSummary[]): RunbookMetricsPresentation {
  const latest = runbooks
    .flatMap((runbook) => runbook.execution.latest ? [runbook.execution.latest] : [])
    .sort((left, right) => right.startedAt.localeCompare(left.startedAt))[0] ?? null;
  return {
    contentRevisions: runbooks.reduce((count, runbook) => count + runbook.contentRevision, 0),
    completedRuns: runbooks.reduce((count, runbook) => count + runbook.execution.completedRunCount, 0),
    executedCells: runbooks.reduce((count, runbook) => count + runbook.execution.executedCellCount, 0),
    latestStatus: latest?.status ?? null
  };
}

function runbookMetricsText(metrics: RunbookMetricsPresentation): string {
  return runbookMetricLabels(metrics).join(' · ');
}

function runbookMetricLabels(metrics: RunbookMetricsPresentation): string[] {
  return [
    `Runbook ${metrics.latestStatus ? traceLabel(metrics.latestStatus) : 'Not Run'}`,
    `${metrics.contentRevisions} Revisions`,
    `${metrics.completedRuns} Runs`,
    `${metrics.executedCells} Cells`
  ];
}

function runbookSummaryMetricsText(metrics: RunbookMetricsPresentation): string {
  return [
    metrics.contentRevisions > 0
      ? `${metrics.contentRevisions} ${metrics.contentRevisions === 1 ? 'Revision' : 'Revisions'}`
      : null,
    metrics.completedRuns > 0
      ? `${metrics.completedRuns} ${metrics.completedRuns === 1 ? 'Run' : 'Runs'}`
      : null
  ].filter((value): value is string => value !== null).join(', ');
}

function claimProjectionSummaryText(leadCount: number, findingCount: number): string {
  return [
    leadCount > 0 ? `${leadCount} ${leadCount === 1 ? 'Lead' : 'Leads'}` : null,
    findingCount > 0 ? `${findingCount} ${findingCount === 1 ? 'Finding' : 'Findings'}` : null
  ].filter((value): value is string => value !== null).join(', ');
}

export function claimRatingSummaryText(claims: readonly HoneycrispFindingSummary[]): string {
  const critical = claims.filter((claim) => claim.rating === 'critical').length;
  const high = claims.filter((claim) => claim.rating === 'high').length;
  const medium = claims.filter((claim) => claim.rating === 'medium').length;
  const low = claims.filter((claim) => claim.rating === 'low').length;
  const informational = claims.filter((claim) => claim.rating === 'informational').length;
  const mediumOrLower = medium + low + informational;
  const compact = critical > 0 && high > 0 && mediumOrLower > 0;
  const mediumOrLowerLabel = medium > 0
    ? `${compact ? 'Med' : 'Medium'}${low + informational > 0 ? '-' : ''}`
    : low > 0
      ? `Low${informational > 0 ? '-' : ''}`
      : 'Info';
  return [
    critical > 0 ? `${critical} ${compact ? 'Crit' : 'Critical'}` : null,
    high > 0 ? `${high} High` : null,
    mediumOrLower > 0 ? `${mediumOrLower} ${mediumOrLowerLabel}` : null
  ].filter((value): value is string => value !== null).join(', ');
}

export function CampaignRunbookCatalogSection({
  runbooks,
  nowMs,
  providerModelCatalog = [],
  selectedRunbookId,
  onOpen
}: {
  runbooks: readonly HoneycrispRunbookSummary[];
  nowMs: number;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  selectedRunbookId: string | null;
  onOpen: (runbookId: string) => void;
}): JSX.Element {
  const sectionLabel = runbooks.length === 1 ? 'Runbook' : 'Runbooks';
  return (
    <section className="runbook-catalog-section campaign-runbook-section" aria-label={`${runbooks.length} ${sectionLabel}`} data-campaign-runbooks>
      <h3>{runbooks.length} {sectionLabel}</h3>
      <div className="runbook-catalog-items">
        {runbooks.map((runbook) => (
          <RunbookCatalogItem
            compactTime
            key={runbook.id}
            nowMs={nowMs}
            providerModelCatalog={providerModelCatalog}
            runbook={runbook}
            selected={selectedRunbookId === runbook.id}
            onOpen={() => onOpen(runbook.id)}
          />
        ))}
      </div>
    </section>
  );
}

export function ReportCatalogItem({ report, nowMs = Date.now(), selected, onOpen }: {
  report: HoneycrispReportSummary;
  nowMs?: number;
  selected: boolean;
  onOpen: () => void;
}): JSX.Element {
  return (
    <button
      type="button"
      className={`runbook-catalog-item report-catalog-item report-status-${stateClass(report.status)} ${selected ? 'selected' : ''}`}
      aria-pressed={selected}
      onClick={onOpen}
    >
      <span className="runbook-catalog-heading">
        <span className="runbook-catalog-primary">
          <FileText className="runbook-catalog-icon" size={16} aria-hidden="true" />
          <span className="runbook-catalog-name">{report.title}</span>
        </span>
        <span className="runbook-catalog-heading-trailing">
          <span className="runbook-catalog-status">{traceLabel(report.status)}</span>
          <CatalogTime value={report.updatedAt} nowMs={nowMs} />
        </span>
      </span>
      {report.summary ? <span className="runbook-catalog-purpose">{runbookDescriptionText(report.summary)}</span> : null}
    </button>
  );
}

function ReportCatalogSection({ reports, label, nowMs, selectedReportId, onOpen }: {
  reports: readonly HoneycrispReportSummary[];
  label: 'Complete' | 'Stale';
  nowMs: number;
  selectedReportId: string | null;
  onOpen: (reportId: string) => void;
}): JSX.Element {
  return (
    <section className="runbook-catalog-section report-catalog-section" aria-label={`${reports.length} ${label}`}>
      <h3>{reports.length} {label}</h3>
      <div className={`runbook-catalog-items ${reports.length === 0 ? 'is-empty' : ''}`}>
        {reports.length > 0 ? reports.map((report) => (
          <ReportCatalogItem key={report.id} report={report} nowMs={nowMs} selected={selectedReportId === report.id} onOpen={() => onOpen(report.id)} />
        )) : (
          <p className="runbook-catalog-empty">{label === 'Complete' ? 'No complete reports yet.' : 'No stale reports yet.'}</p>
        )}
      </div>
    </section>
  );
}

export function SubagentCatalogSection({
  agents,
  nowMs = Date.now(),
  providerModelCatalog,
  label,
  selectedPath,
  onSelect
}: {
  agents: readonly SubagentSummary[];
  nowMs?: number;
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
  label: 'Active' | 'Completed';
  selectedPath: string | null;
  onSelect: (path: string) => void;
}): JSX.Element {
  return (
    <section className="subagent-catalog-section" aria-label={`${agents.length} ${label}`}>
      <h3>{agents.length} {label}</h3>
      <div className={`subagent-catalog-items ${agents.length === 0 ? 'is-empty' : ''}`}>
        {agents.length > 0 ? (
          agents.map((agent) => (
            <button
              type="button"
              className={`subagent-catalog-item ${selectedPath === agent.path ? 'selected' : ''}`}
              aria-pressed={selectedPath === agent.path}
              key={agent.path}
              onClick={() => onSelect(agent.path)}
            >
              <span className="subagent-catalog-heading">
                <span className="subagent-catalog-identity">
                  <SubagentStatusIndicator status={agent.status} variant="robot" />
                  <strong className="subagent-catalog-name">{subagentDisplayName(agent.name)}</strong>
                </span>
                <span className="subagent-catalog-heading-trailing">
                  <CatalogTime value={agent.createdAt} nowMs={nowMs} />
                </span>
              </span>
              <span className="subagent-catalog-preview">{agent.latestMessage || 'No message yet.'}</span>
              <span className="subagent-catalog-footer">
                <span className="subagent-catalog-channel">{subagentChannelLabel(agent.channelName)}</span>
                <span className="subagent-catalog-model-identity">
                  <SubagentProviderIcon provider={agent.provider} model={agent.model} />
                  <span className="subagent-catalog-model">{subagentModelDisplayName(agent.provider, agent.model, providerModelCatalog)}</span>
                </span>
              </span>
            </button>
          ))
        ) : (
          <p className="subagent-catalog-empty">
            {label === 'Active' ? 'No active subagents right now.' : 'No completed subagents yet.'}
          </p>
        )}
      </div>
    </section>
  );
}

export function subagentModelDisplayName(
  provider: string | null,
  model: string | null,
  catalogs: readonly ResearchProviderModelCatalog[]
): string {
  return researchModelDisplayName(provider, model, catalogs);
}

function SubagentProviderIcon({
  provider,
  model
}: Pick<SubagentSummary, 'provider' | 'model'>): JSX.Element {
  const modelLabel = model ?? 'Unknown model';
  return (
    <span className="subagent-provider-icon" aria-label={`Model: ${modelLabel}`} title={modelLabel}>
      <ProviderIcon provider={provider ?? model} size={15} aria-hidden="true" />
    </span>
  );
}

function SubagentStatusIndicator({
  status,
  variant = 'dot'
}: {
  status: SubagentStatus | 'unknown';
  variant?: 'dot' | 'robot';
}): JSX.Element {
  const iconKind = status === 'unknown' ? 'unknown' : subagentStatusIconKind(status);
  const label = status === 'unknown' ? 'Unknown' : subagentStatusLabel(status);
  return (
    <span
      className={`subagent-catalog-status uses-${variant} is-${iconKind}`}
      role="img"
      aria-label={label}
      title={label}
    >
      {iconKind === 'active'
        ? <LoaderCircle className="subagent-catalog-status-spinner" size={variant === 'robot' ? 15 : 13} aria-hidden="true" />
        : variant === 'robot'
          ? <Bot className="subagent-catalog-status-robot" size={15} aria-hidden="true" />
          : null}
    </span>
  );
}

export function MemoryCatalogItem({
  node,
  compactTime = false,
  memoryStatuses,
  memoryTypes,
  nowMs = Date.now(),
  profileId,
  providerModelCatalog = [],
  sessionHeatPreferences = EMPTY_SESSION_HEAT_PREFERENCES,
  selected,
  onOpen
}: {
  node: HoneycrispMemoryNodeSummary;
  compactTime?: boolean;
  memoryStatuses?: readonly ResearchProfileMemoryStatus[];
  memoryTypes?: readonly ResearchProfileMemoryType[];
  nowMs?: number;
  profileId?: string | null;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  sessionHeatPreferences?: SessionHeatPreferences;
  selected: boolean;
  onOpen: () => void;
}): JSX.Element {
  const statusLabel = memoryStatuses?.find((status) => status.id === node.status)?.name ?? traceLabel(node.status);
  return (
    <article className={`memory-catalog-item type-${stateClass(node.type)} ${compactTime ? 'is-compact' : ''} ${selected ? 'selected' : ''}`}>
      <button type="button" className="memory-catalog-toggle" aria-pressed={selected} onClick={onOpen}>
        <span className="memory-catalog-item-heading">
          {compactTime ? (
            <>
              <span className="memory-catalog-item-meta-line">
                <span className="memory-catalog-item-trailing">
                  <CatalogTime value={node.updatedAt} nowMs={nowMs} />
                </span>
                <span className="memory-catalog-item-primary">
                  <span className="memory-catalog-item-name" title={node.title}>
                    <Database
                      className={`memory-catalog-title-icon memory-status-${memoryStatusPolarity(node.status, memoryStatuses ?? [])}`}
                      size={15}
                      aria-hidden="true"
                    />
                    {node.title}
                  </span>
                </span>
              </span>
              <span className="memory-catalog-item-context">
                <MemoryTypeLabel
                  className="memory-catalog-item-type"
                  type={node.type}
                  definitions={memoryTypes}
                  label={`${memoryTypeLabel(node.type, memoryTypes)} ${statusLabel}`}
                  profileId={profileId}
                  sessionHeatPreferences={sessionHeatPreferences}
                  showDot={false}
                />
                <MemoryCatalogAuthors authors={node.authors} providerModelCatalog={providerModelCatalog} />
              </span>
            </>
          ) : (
            <>
              <span className="memory-catalog-item-meta-line">
                <span className="memory-catalog-item-primary">
                  <span className="memory-catalog-item-name" title={node.title}>{node.title}</span>
                </span>
                <span className="memory-catalog-item-trailing">
                  <time dateTime={node.updatedAt} title={formatSessionDateTime(node.updatedAt)}>{formatSessionDateTime(node.updatedAt)}</time>
                </span>
              </span>
              {node.summary || node.body ? (
                <span className="memory-catalog-item-description">{renderInlineCodeText(node.summary || node.body)}</span>
              ) : null}
            </>
          )}
        </span>
      </button>
    </article>
  );
}

function MemoryCatalogAuthors({
  authors,
  providerModelCatalog
}: {
  authors: HoneycrispMemoryNodeSummary['authors'];
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
}): JSX.Element | null {
  if (!authors?.length) return null;
  return (
    <span className="memory-catalog-item-authors" aria-label="Model authors">
      {authors.map((author) => {
        const modelName = subagentModelDisplayName(author.provider, author.model, providerModelCatalog);
        return (
          <span className="memory-catalog-item-author" key={`${author.provider}\0${author.model}`} title={`${author.provider}/${author.model}`}>
            <span className="memory-catalog-item-author-provider">
              <ProviderIcon provider={author.provider || author.model} size={15} aria-hidden="true" />
            </span>
            <span className="memory-catalog-item-author-model">{modelName}</span>
          </span>
        );
      })}
    </span>
  );
}

export function CampaignClaimCatalogLists({
  findings,
  leads,
  nowMs = Date.now(),
  previewLimit,
  providerModelCatalog = [],
  showEmptySections = false,
  selectedClaimId,
  onOpen
}: {
  findings: readonly HoneycrispFindingSummary[];
  leads: readonly HoneycrispFindingSummary[];
  nowMs?: number;
  previewLimit?: number;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  showEmptySections?: boolean;
  selectedClaimId: string | null;
  onOpen: (claimId: string) => void;
}): JSX.Element {
  return (
    <>
      {showEmptySections || findings.length > 0 ? (
        <CampaignClaimCatalogSection
          claims={findings}
          emptyLabel={showEmptySections ? 'No workspace findings yet.' : undefined}
          label="Findings"
          nowMs={nowMs}
          previewLimit={previewLimit}
          providerModelCatalog={providerModelCatalog}
          selectedClaimId={selectedClaimId}
          onOpen={onOpen}
        />
      ) : null}
      {showEmptySections || leads.length > 0 ? (
        <CampaignClaimCatalogSection
          claims={leads}
          emptyLabel={showEmptySections ? 'No workspace leads yet.' : undefined}
          label="Leads"
          nowMs={nowMs}
          previewLimit={previewLimit}
          providerModelCatalog={providerModelCatalog}
          selectedClaimId={selectedClaimId}
          onOpen={onOpen}
        />
      ) : null}
    </>
  );
}

export function CampaignClaimCatalogSection({
  claims,
  emptyLabel,
  label,
  nowMs = Date.now(),
  previewLimit,
  providerModelCatalog = [],
  selectedClaimId,
  onOpen
}: {
  claims: readonly HoneycrispFindingSummary[];
  emptyLabel?: string;
  label: 'Findings' | 'Leads';
  nowMs?: number;
  previewLimit?: number;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  selectedClaimId: string | null;
  onOpen: (claimId: string) => void;
}): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const boundedPreviewLimit = previewLimit === undefined ? claims.length : Math.max(0, previewLimit);
  const visibleClaims = claims.slice(0, boundedPreviewLimit);
  const overflowClaims = claims.slice(boundedPreviewLimit);
  const sectionLabel = claims.length === 1 ? singularizePresentationLabel(label) : label;
  const unrefutedClaimCount = label === 'Findings'
    ? claims.filter((claim) => claim.maturity !== 'refuted').length
    : null;
  const renderClaim = (claim: HoneycrispFindingSummary): JSX.Element => {
    const rating = campaignClaimCatalogRatingPresentation(claim);
    return (
      <article className={`memory-catalog-item campaign-claim-item is-compact is-${claim.projection} ${selectedClaimId === claim.id ? 'selected' : ''}`} key={claim.id}>
        <button type="button" className="memory-catalog-toggle" aria-pressed={selectedClaimId === claim.id} onClick={() => onOpen(claim.id)}>
          <span className="memory-catalog-item-heading">
            <span className="memory-catalog-item-meta-line">
              <span className="memory-catalog-item-trailing">
                <CatalogTime value={claim.updatedAt} nowMs={nowMs} />
              </span>
              <span className="memory-catalog-item-primary">
                <span className="memory-catalog-item-name" title={claim.title}>
                  <CampaignClaimProjectionIcon
                    className={`campaign-claim-title-icon maturity-${stateClass(claim.maturity)}`}
                    projection={claim.projection}
                  />
                  {claim.title}
                </span>
              </span>
            </span>
            <span className="memory-catalog-item-context">
              <span
                className={`campaign-claim-context-label campaign-claim-kind campaign-claim-rating-class is-${claim.projection}`}
                title={`${rating.title}; Maturity: ${traceLabel(claim.maturity)}`}
              >
                {rating.label}, {traceLabel(claim.maturity)}
              </span>
              <MemoryCatalogAuthors authors={claim.authors} providerModelCatalog={providerModelCatalog} />
            </span>
          </span>
        </button>
      </article>
    );
  };
  return (
    <section
      className="memory-type-section campaign-claim-section"
      aria-label={`${claims.length} ${sectionLabel}${unrefutedClaimCount === null ? '' : `, ${unrefutedClaimCount} unrefuted`}`}
      data-campaign-projection={label.toLocaleLowerCase()}
    >
      <h3>
        {claims.length.toLocaleString()} {sectionLabel}
        {unrefutedClaimCount === null ? null : ` (${unrefutedClaimCount.toLocaleString()} Unrefuted)`}
      </h3>
      <div className="memory-type-items">
        <div className="workspace-claim-primary-items">{visibleClaims.map(renderClaim)}</div>
        {overflowClaims.length > 0 ? (
          <div
            aria-hidden={!expanded}
            className={`workspace-claim-overflow ${expanded ? 'expanded' : ''}`.trim()}
            inert={expanded ? undefined : true}
          >
            <div>{overflowClaims.map(renderClaim)}</div>
          </div>
        ) : null}
        {claims.length === 0 && emptyLabel ? <p className="workspace-catalog-empty">{emptyLabel}</p> : null}
      </div>
      {overflowClaims.length > 0 ? (
        <button
          aria-expanded={expanded}
          className="session-memory-type-toggle workspace-memory-type-toggle workspace-claim-toggle"
          onClick={() => setExpanded((current) => !current)}
          type="button"
        >
          {expanded ? 'Show less' : `Show ${overflowClaims.length.toLocaleString()} more`}
        </button>
      ) : null}
    </section>
  );
}

export function cvssQualitativeSeverityLabel(score: number): 'None' | 'Low' | 'Medium' | 'High' | 'Critical' {
  if (score <= 0) return 'None';
  if (score < 4) return 'Low';
  if (score < 7) return 'Medium';
  if (score < 9) return 'High';
  return 'Critical';
}

function campaignClaimCatalogRatingPresentation(claim: HoneycrispFindingSummary): {
  label: string;
  title: string;
} {
  const latestCvss = claim.securityTracking?.cvssAssessments.at(-1) ?? null;
  if (!latestCvss) {
    const label = traceLabel(claim.rating);
    return { label, title: `Untrusted rating: ${label}` };
  }
  const score = latestCvss.score.toFixed(1);
  const label = `${cvssQualitativeSeverityLabel(latestCvss.score)} (CVSS ${score})`;
  return { label, title: `CVSS rating: ${label}` };
}

export function campaignClaimTypeLabel(classification: string, fallback: string): string {
  const unqualified = classification.trim().split('.').filter(Boolean).at(-1);
  return unqualified
    ? traceLabel(unqualified.toLocaleLowerCase().replaceAll('-', '_'))
    : fallback;
}

function CampaignClaimProjectionIcon({
  className,
  projection
}: {
  className?: string;
  projection: 'lead' | 'finding';
}): JSX.Element {
  return projection === 'finding'
    ? <BadgeCheck className={className} size={15} aria-hidden="true" />
    : <Lightbulb className={className} size={15} aria-hidden="true" />;
}

export function MemoryTypeCatalogSection({
  nodes,
  type,
  memoryTypes,
  memoryStatuses,
  nowMs = Date.now(),
  profileId,
  providerModelCatalog = [],
  sessionHeatPreferences = EMPTY_SESSION_HEAT_PREFERENCES,
  expanded,
  selectedNodeId,
  onExpand,
  onOpen
}: {
  nodes: readonly HoneycrispMemoryNodeSummary[];
  type: string;
  memoryTypes?: readonly ResearchProfileMemoryType[];
  memoryStatuses?: readonly ResearchProfileMemoryStatus[];
  nowMs?: number;
  profileId?: string | null;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  sessionHeatPreferences?: SessionHeatPreferences;
  expanded: boolean;
  selectedNodeId: string | null;
  onExpand: () => void;
  onOpen: (nodeId: string) => void;
}): JSX.Element {
  const { visibleNodes, hiddenCount } = memoryCatalogGroupPreview(nodes, expanded);
  const definition = memoryTypeDefinition(type, memoryTypes);
  const fallbackLabel = memoryTypeLabel(type, memoryTypes);
  const sectionLabel = nodes.length === 1
    ? definition?.name ?? fallbackLabel
    : definition?.pluralName ?? pluralizePresentationLabel(fallbackLabel);
  return (
    <section className="memory-type-section" aria-label={`${nodes.length} ${sectionLabel}`} data-memory-type={type}>
      <h3>{nodes.length.toLocaleString()} {sectionLabel}</h3>
      <div className={`memory-type-items ${nodes.length === 0 ? 'is-empty' : ''}`}>
        {visibleNodes.length > 0 ? visibleNodes.map((node) => (
          <MemoryCatalogItem
            compactTime
            key={node.id}
            node={node}
            memoryStatuses={memoryStatuses}
            memoryTypes={memoryTypes}
            nowMs={nowMs}
            profileId={profileId}
            providerModelCatalog={providerModelCatalog}
            sessionHeatPreferences={sessionHeatPreferences}
            selected={selectedNodeId === node.id}
            onOpen={() => onOpen(node.id)}
          />
        )) : (
          <p className="memory-type-empty">No {sectionLabel.toLocaleLowerCase()} yet.</p>
        )}
      </div>
      {hiddenCount > 0 ? (
        <button type="button" className="memory-type-show-more" onClick={onExpand}>
          <span>Show {hiddenCount} More</span>
          <ChevronDown size={14} aria-hidden="true" />
        </button>
      ) : null}
    </section>
  );
}

export function MemoryDetailView({
  node,
  nodeById,
  relationships,
  researchProfile = null,
  sessionHeatPreferences = EMPTY_SESSION_HEAT_PREFERENCES
}: {
  node: HoneycrispMemoryNodeSummary;
  nodeById: Map<string, HoneycrispMemoryNodeSummary>;
  relationships: HoneycrispMemoryEdgeSummary[];
  researchProfile?: ResearchProfile | null;
  sessionHeatPreferences?: SessionHeatPreferences;
}): JSX.Element {
  const memoryProfile = researchProfile?.memory;
  const statusDefinition = memoryProfile?.statuses.find((status) => status.id === node.status);
  const statusLabel = statusDefinition?.name
    ?? (memoryProfile ? unknownProfileValueLabel('status', node.status) : traceLabel(node.status));
  const relationDefinitions = new Map(memoryProfile?.relations?.map((relation) => [relation.id, relation]) ?? []);
  const evidenceKindDefinitions = new Map(memoryProfile?.evidenceKinds.map((kind) => [kind.id, kind]) ?? []);
  const workspaceNoun = researchProfile?.workspace.workspaceNoun ?? 'Workspace';
  const subjectNoun = researchProfile?.workspace.subjectNoun ?? 'Subject';
  const sessionLabel = researchProfile?.presentation.sessionLabel ?? 'Session';
  return (
    <article className={`memory-detail type-${stateClass(node.type)}`}>
      <header className="memory-detail-heading">
        <span className="memory-catalog-item-labels">
          <MemoryTypeLabel
            type={node.type}
            definitions={memoryProfile?.types}
            status={node.status}
            profileId={researchProfile?.id}
            sessionHeatPreferences={sessionHeatPreferences}
          />
          <span className="memory-catalog-status">{statusLabel}</span>
        </span>
        <time dateTime={node.updatedAt} title={formatSessionDateTime(node.updatedAt)}>{formatSessionDateTime(node.updatedAt)}</time>
        <h3>{node.title}</h3>
        <ModelAuthors authors={node.authors} />
      </header>
      <div className="memory-catalog-content">
        {node.summary ? <p className="memory-catalog-summary">{node.summary}</p> : null}
        {node.body && node.body !== node.summary ? <p className="memory-catalog-body">{node.body}</p> : null}
        <div className="memory-catalog-meta">
          <span>Update {node.revision}</span>
          <span>{node.evidenceRefs.length} refs</span>
          <span>{relationships.length} links</span>
        </div>
        <dl className="memory-catalog-scope">
          <div><dt>{subjectNoun}</dt><dd>{node.subjectName}</dd></div>
          <div><dt>{pluralizePresentationLabel(workspaceNoun)}</dt><dd>{node.workspaces.map((workspace) => workspace.name).join(', ') || 'None'}</dd></div>
          <div><dt>{pluralizePresentationLabel(sessionLabel)}</dt><dd>{node.sessionIds.join(', ') || 'None'}</dd></div>
        </dl>
        {node.assetIds.length > 0 ? <ChipGroup label="Assets" values={node.assetIds} /> : null}
        {node.tags.length > 0 ? <ChipGroup label="Tags" values={node.tags} /> : null}
        {node.evidenceRefs.length > 0 ? (
          <section className="memory-catalog-subsection" aria-label="References">
            <h4>References</h4>
            <div className="memory-reference-list">
              {node.evidenceRefs.map((reference) => (
                <article key={reference.id}>
                  <span>{evidenceKindDefinitions.get(reference.kind)?.name
                    ?? (memoryProfile ? unknownProfileValueLabel('evidence kind', reference.kind) : traceLabel(reference.kind))}</span>
                  <strong>{reference.summary || reference.path || reference.id}</strong>
                  {reference.path ? <code>{reference.path}</code> : null}
                </article>
              ))}
            </div>
          </section>
        ) : null}
        {relationships.length > 0 ? (
          <section className="memory-catalog-subsection" aria-label="Relationships">
            <h4>Relationships</h4>
            <div className="memory-relationship-list">
              {relationships.map((relationship) => {
                const outbound = relationship.fromId === node.id;
                const relatedId = outbound ? relationship.toId : relationship.fromId;
                const relatedNode = nodeById.get(relatedId);
                return (
                  <article key={`${relationship.fromId}:${relationship.relation}:${relationship.toId}`}>
                    <span>{outbound ? '→' : '←'} {relationDefinitions.get(relationship.relation)?.name
                      ?? (memoryProfile ? unknownProfileValueLabel('relation', relationship.relation) : traceLabel(relationship.relation))}</span>
                    <strong>{relatedNode?.title ?? relatedId}</strong>
                    {relationship.note ? <p>{relationship.note}</p> : null}
                  </article>
                );
              })}
            </div>
          </section>
        ) : null}
      </div>
    </article>
  );
}

export function CampaignClaimDetailView({
  claim,
  nowMs = Date.now(),
  providerModelCatalog = []
}: {
  claim: HoneycrispFindingSummary;
  nowMs?: number;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
}): JSX.Element {
  const projectionLabel = claim.projection === 'finding' ? 'Finding' : 'Lead';
  const confidence = `${Math.round(Math.max(0, Math.min(1, claim.confidence)) * 100)}% confidence`;
  const securityTracking = claim.securityTracking;
  const latestCvss = securityTracking?.cvssAssessments.at(-1) ?? null;
  const latestRiskDecision = securityTracking?.riskDecisions.at(-1) ?? null;
  return (
    <article className={`memory-detail campaign-claim-detail is-${claim.projection}`}>
      <header className="memory-detail-heading">
        <h3>
          <CampaignClaimProjectionIcon
            className={`campaign-claim-title-icon campaign-claim-detail-title-icon maturity-${stateClass(claim.maturity)}`}
            projection={claim.projection}
          />
          {claim.title}{' '}
          <CatalogTime value={claim.updatedAt} nowMs={nowMs} />
        </h3>
        <span className="memory-catalog-item-labels campaign-claim-detail-label">
          <span className={`campaign-claim-kind is-${claim.projection}`}>
            {projectionLabel} {traceLabel(claim.maturity)}, {traceLabel(claim.rating)} {campaignClaimTypeLabel(claim.classification, projectionLabel)}
          </span>
        </span>
        <MemoryCatalogAuthors authors={claim.authors} providerModelCatalog={providerModelCatalog} />
      </header>
      <div className="memory-catalog-content">
        <section className="campaign-claim-assessment" aria-label="Overview, impact, and reachability">
          <div>
            <strong>Overview</strong>
            {claim.summary.trim() ? <p>{claim.summary}</p> : <em>Unknown</em>}
          </div>
          <div>
            <strong>Impact</strong>
            {claim.impact.trim() ? <p>{claim.impact}</p> : <em>Unknown</em>}
          </div>
          <div>
            <strong>Reachability</strong>
            {securityTracking?.reachability.conditions?.trim()
              ? <p>{securityTracking.reachability.conditions}</p>
              : <em>Unknown</em>}
          </div>
        </section>
        <div className="memory-catalog-meta">
          <span>Update {claim.revision}</span>
          <span>{confidence}</span>
          <span>{claim.evidence.length} refs</span>
        </div>
        <dl className="memory-catalog-scope campaign-claim-scope">
          <ClaimDetailAttribute title="Class">
            {claim.classification || 'Unclassified'}
          </ClaimDetailAttribute>
          <ClaimDetailAttribute title="Untrusted rating">
            {traceLabel(claim.rating)}
          </ClaimDetailAttribute>
          <ClaimDetailAttribute title="Maturity">
            {traceLabel(claim.maturity)}
          </ClaimDetailAttribute>
          <ClaimDetailAttribute title="Workflow">
            {traceLabel(claim.workflow)}
          </ClaimDetailAttribute>
          <ClaimDetailAttribute title="Freshness">
            {traceLabel(claim.freshness)}
          </ClaimDetailAttribute>
          {securityTracking ? (
            <ClaimDetailAttribute title="Reachability">
              {traceLabel(securityTracking.reachability.state)}
            </ClaimDetailAttribute>
          ) : null}
          {securityTracking ? (
            <ClaimDetailAttribute title="Risk treatment">
              {traceLabel(securityTracking.riskTreatment)}
            </ClaimDetailAttribute>
          ) : null}
          {latestCvss ? (
            <ClaimDetailAttribute title="CVSS">
              {latestCvss.score.toFixed(1)} · {latestCvss.nomenclature}
            </ClaimDetailAttribute>
          ) : null}
          <ClaimDetailAttribute title="Workspace">
            {claim.workspaceId}
          </ClaimDetailAttribute>
          <ClaimDetailAttribute title="Subject">
            {claim.subjectId}
          </ClaimDetailAttribute>
          <ClaimDetailAttribute title="Origin">
            {claim.originSessionId ?? 'None'}
          </ClaimDetailAttribute>
        </dl>
        {claim.componentClaimIds.length > 0 ? <ChipGroup label="Components" values={claim.componentClaimIds} /> : null}
        {securityTracking && securityTracking.affectedAssetIds.length > 0 ? <ChipGroup label="Affected assets" values={securityTracking.affectedAssetIds} /> : null}
        {latestRiskDecision ? (
          <p className="memory-catalog-body">
            <strong>Risk decision:</strong> {latestRiskDecision.rationale}
            {latestRiskDecision.expiresAt ? ` Expires ${formatSessionDateTime(latestRiskDecision.expiresAt)}.` : ''}
          </p>
        ) : null}
        {claim.staleReason ? <p className="memory-catalog-body"><strong>Stale reason:</strong> {claim.staleReason}</p> : null}
        {securityTracking && securityTracking.cvssAssessments.length > 0 ? (
          <section className="memory-catalog-subsection" aria-label="CVSS assessments">
            <h4>CVSS Assessments</h4>
            <div className="memory-reference-list">
              {[...securityTracking.cvssAssessments].reverse().map((assessment) => (
                <article key={`${assessment.version}:${assessment.vector}:${assessment.assessedAt}`}>
                  <span>{assessment.nomenclature} · {assessment.score.toFixed(1)}</span>
                  <strong>CVSS {assessment.version}</strong>
                  <code>{assessment.vector}</code>
                </article>
              ))}
            </div>
          </section>
        ) : null}
        {securityTracking && securityTracking.affectedVersions.length > 0 ? (
          <section className="memory-catalog-subsection" aria-label="Affected versions">
            <h4>Affected Versions</h4>
            <div className="memory-reference-list">
              {securityTracking.affectedVersions.map((version) => (
                <article key={`${version.assetId ?? 'claim'}:${version.range}:${version.fixedVersion ?? ''}`}>
                  <span>{version.assetId ?? 'Finding'}</span>
                  <strong>{version.range}</strong>
                  {version.fixedVersion ? <code>Fixed in {version.fixedVersion}</code> : null}
                </article>
              ))}
            </div>
          </section>
        ) : null}
        {securityTracking && securityTracking.externalReferences.length > 0 ? (
          <section className="memory-catalog-subsection" aria-label="External references">
            <h4>External References</h4>
            <div className="memory-reference-list">
              {securityTracking.externalReferences.map((reference) => (
                <article key={`${reference.kind}:${reference.identifier}`}>
                  <span>{traceLabel(reference.kind)}</span>
                  <strong>{reference.identifier}</strong>
                  {reference.url ? <code>{reference.url}</code> : null}
                </article>
              ))}
            </div>
          </section>
        ) : null}
        {claim.evidence.length > 0 ? (
          <section className="memory-catalog-subsection" aria-label="References">
            <h4>References</h4>
            <div className="memory-reference-list">
              {claim.evidence.map((evidence) => (
                <article key={evidence.id}>
                  <span>{traceLabel(evidence.kind)}{evidence.independent ? ' · Independent' : ''}</span>
                  <strong>{evidence.summary || evidence.referenceId || evidence.id}</strong>
                  {evidence.referenceId ? <code>{evidence.referenceId}</code> : null}
                </article>
              ))}
            </div>
          </section>
        ) : null}
        {claim.transitions.length > 0 ? (
          <section className="memory-catalog-subsection" aria-label="History">
            <h4>History</h4>
            <div className="memory-relationship-list">
              {[...claim.transitions].reverse().map((transition) => (
                <article key={transition.id}>
                  <span>{transition.fromStatus ? `${traceLabel(transition.fromStatus)} → ` : ''}{traceLabel(transition.toStatus)}</span>
                  <strong>{transition.reason || `Update ${transition.revision}`}</strong>
                  <time dateTime={transition.createdAt}>{formatSessionDateTime(transition.createdAt)}</time>
                </article>
              ))}
            </div>
          </section>
        ) : null}
      </div>
    </article>
  );
}

function ClaimDetailAttribute({
  children,
  title
}: {
  children: ReactNode;
  title: string;
}): JSX.Element {
  return (
    <div>
      <dt>
        <strong>{title}</strong>
      </dt>
      <dd>{children}</dd>
    </div>
  );
}

function ChipGroup({ label, values }: { label: string; values: string[] }): JSX.Element {
  return (
    <div className="memory-chip-group">
      <span>{label}</span>
      <div>{values.map((value) => <span key={value}>{value}</span>)}</div>
    </div>
  );
}
