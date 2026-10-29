import { useEffect, useMemo, useRef, useState } from 'react';
import type { FormEvent, JSX } from 'react';
import { BookOpen, Brain, Columns3, FileText, GitBranch, Info, ListChecks, Network, Plus, RefreshCw, Server, Settings, Trash2 } from 'lucide-react';
import type { BealeWorkspaceResearchTrack, BealeWorkspaceSurfaceMap } from '@beale/app-server-runtime/protocol';
import { isLiveResearchRunStatus, repositoryClonedDirectory } from '../../../shared/types';
import { researchKitDefinition, researchKitLabel, researchKitResourceKey } from '../../../shared/researchKits';
import type {
  AppServerMemorySummary,
  AppServerFindingSummary,
  ClaimBoardMaturity,
  AppServerReportSummary,
  ResearchProfile,
  ResearchKitId,
  ResearchKitRefreshInput,
  ResearchKitRefreshResult,
  ResearchProviderModelCatalog,
  RunRow,
  ScopeAsset,
  ScopeAssetInput,
  ScopeAssetKind,
  WorkspaceMemoryBackendId,
  WorkspaceScopeDraft,
  WorkspaceScopeVersion,
  WorkspaceRule
} from '@shared/types';
import { Modal } from '../../app/Modal';
import { MainSideScrollRegion } from '../../app/MainSideScrollRegion';
import { memoryTypeDefinition, memoryTypeLabel } from '../research/MemoryTypeLabel';
import { MemoryCatalogItem, ReportCatalogItem, RunbookCatalogItem } from '../research/MemorySidePanel';
import { memoryTypeGroupsByHeat } from '../../view-models/memoryCatalog';
import { EMPTY_SESSION_HEAT_PREFERENCES } from '../../view-models/sessionHeat';
import type { SessionHeatPreferences } from '../../view-models/sessionHeat';
import type { MapSurface } from '../../view-models/surfaceMap';
import { errorMessage } from '../../lib/errors';
import { renderTraceProseText } from '../traces/traceMarkup';
import { WorkspaceDirectoriesField } from './WorkspaceDirectoriesWidget';
import { WorkspaceTargetsPanel } from './WorkspaceTargetsPanel';
import { CampaignBoardView } from './CampaignGraphView';
import { WorkspaceSurfaceMap } from './WorkspaceSurfaceMap';
import { WORKSPACE_ASSET_KINDS, WorkspaceAssetIcon, workspaceAssetKindLabel } from './WorkspaceResourceKinds';
export { WORKSPACE_ASSET_KINDS, WorkspaceAssetIcon, workspaceAssetKindLabel } from './WorkspaceResourceKinds';
import { WorkspaceResearchTrack } from './WorkspaceResearchTrack';
import { WorkspaceTitleSearch, workspaceCatalogMatchesQuery } from './WorkspaceTitleSearch';

const WORKSPACE_ACTIVITY_DAY_COUNT = 365;
const DAY_DURATION_MS = 24 * 60 * 60 * 1_000;
const WORKSPACE_DASHBOARD_VIEWS = ['campaign', 'surfaces', 'rules', 'targets', 'overview', 'kit'] as const;
const WORKSPACE_CAMPAIGN_VIEWS = ['board', 'memory', 'runbooks', 'reports'] as const;

type WorkspaceTopLevelView = typeof WORKSPACE_DASHBOARD_VIEWS[number];
type WorkspaceCampaignView = typeof WORKSPACE_CAMPAIGN_VIEWS[number];
export type WorkspaceDashboardView = WorkspaceTopLevelView | WorkspaceCampaignView | 'activity' | 'resources';

const WORKSPACE_DASHBOARD_VIEW_ICONS: Record<WorkspaceTopLevelView, typeof Info> = {
  overview: Settings,
  targets: Server,
  campaign: GitBranch,
  surfaces: Network,
  kit: RefreshCw,
  rules: ListChecks
};

const WORKSPACE_CAMPAIGN_VIEW_ICONS: Record<WorkspaceCampaignView, typeof Info> = {
  board: Columns3,
  memory: Brain,
  runbooks: BookOpen,
  reports: FileText
};

export interface WorkspaceConfigurationInput {
  workspaceName: string;
  descriptionMarkdown: string;
}

export interface WorkspaceHeatmapDay {
  dateKey: string;
  timestamp: number;
  value: number;
  heatLevel: 0 | 1 | 2 | 3 | 4;
}

export interface WorkspaceHeatmapActivity {
  days: WorkspaceHeatmapDay[];
  leadingEmptyDays: number;
  total: number;
}

export interface WorkspaceTokenActivityDay extends WorkspaceHeatmapDay {
  totalTokens: number;
}

export interface WorkspaceTokenActivity extends WorkspaceHeatmapActivity {
  days: WorkspaceTokenActivityDay[];
  totalTokens: number;
}

export function workspaceTokenActivity(runs: readonly RunRow[], nowMs: number): WorkspaceTokenActivity {
  const activity = workspaceDailyActivity(runs.map(({ run, tokenUsage }) => ({
    occurredAt: run.endedAt ?? run.startedAt ?? run.createdAt,
    value: tokenUsage?.totalTokens ?? 0
  })), nowMs);
  return {
    ...activity,
    days: activity.days.map((day) => ({ ...day, totalTokens: day.value })),
    totalTokens: activity.total
  };
}

export function workspaceCreationActivity(
  items: readonly { createdAt: string }[],
  nowMs: number
): WorkspaceHeatmapActivity {
  return workspaceDailyActivity(items.map((item) => ({ occurredAt: item.createdAt, value: 1 })), nowMs);
}

function workspaceDailyActivity(
  events: readonly { occurredAt: string; value: number }[],
  nowMs: number
): WorkspaceHeatmapActivity {
  const end = startOfUtcDay(nowMs);
  const start = end - ((WORKSPACE_ACTIVITY_DAY_COUNT - 1) * DAY_DURATION_MS);
  const totalsByDate = new Map<string, number>();
  for (const event of events) {
    if (event.value <= 0) continue;
    const activityAt = Date.parse(event.occurredAt);
    if (!Number.isFinite(activityAt) || activityAt < start || activityAt >= end + DAY_DURATION_MS) continue;
    const dateKey = utcDateKey(activityAt);
    totalsByDate.set(dateKey, (totalsByDate.get(dateKey) ?? 0) + event.value);
  }
  const maximum = Math.max(0, ...totalsByDate.values());
  const days = Array.from({ length: WORKSPACE_ACTIVITY_DAY_COUNT }, (_, index): WorkspaceHeatmapDay => {
    const timestamp = start + (index * DAY_DURATION_MS);
    const dateKey = utcDateKey(timestamp);
    const value = totalsByDate.get(dateKey) ?? 0;
    return {
      dateKey,
      timestamp,
      value,
      heatLevel: workspaceHeatLevel(value, maximum)
    };
  });
  return {
    days,
    leadingEmptyDays: new Date(start).getUTCDay(),
    total: days.reduce((total, day) => total + day.value, 0)
  };
}

function workspaceHeatLevel(value: number, maximum: number): 0 | 1 | 2 | 3 | 4 {
  if (value <= 0 || maximum <= 0) return 0;
  return Math.max(1, Math.min(4, Math.ceil((Math.log1p(value) / Math.log1p(maximum)) * 4))) as 1 | 2 | 3 | 4;
}

function startOfUtcDay(timestamp: number): number {
  const date = new Date(timestamp);
  return Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate());
}

function utcDateKey(timestamp: number): string {
  return new Date(timestamp).toISOString().slice(0, 10);
}

export function workspaceScopeDraftForConfigurationUpdate(
  scope: WorkspaceScopeVersion,
  configuration: WorkspaceConfigurationInput
): WorkspaceScopeDraft {
  return {
    workspaceName: configuration.workspaceName,
    scopeOwner: scope.scopeOwner,
    descriptionMarkdown: configuration.descriptionMarkdown,
    rulesMarkdown: '',
    expiresAt: scope.expiresAt,
    assets: scope.assets.map((asset) => ({
      direction: asset.direction,
      kind: asset.kind,
      value: asset.value,
      sensitivity: asset.sensitivity,
      attributes: asset.attributes
    }))
  };
}

export function WorkspaceUnderstandingView({
  busy,
  appServerMemory,
  activeScope = null,
  workspaceRules = [],
  researchProfile = null,
  researchKitId = 'general',
  sessionHeatPreferences = EMPTY_SESSION_HEAT_PREFERENCES,
  researchSubjectName = '',
  workspaceId: workspaceIdInput = '',
  workspacePath = '',
  workspaceDirectories,
  memoryBackend = 'app-server',
  providerModelCatalog = [],
  workspaceName,
  runs,
  onAddResource = async () => undefined,
  onRefreshResearchKit = async () => { throw new Error('Research Kit refresh is unavailable.'); },
  onAddRule = async () => undefined,
  onSaveConfiguration = async () => undefined,
  onChangeWorkspaceDirectories = async () => undefined,
  onChangeMemoryBackend = async () => undefined,
  onChangeResearchSubject = async () => undefined,
  onRemoveWorkspace = async () => undefined,
  onOpenSession,
  onStartTrack,
  onTrackResumed,
  onOpenClaim = () => undefined,
  openClaimId = null,
  onTransitionClaim,
  onOpenMemory = () => undefined,
  onOpenRunbook = () => undefined,
  onOpenReport = () => undefined,
  onActiveViewChange,
  onViewSelectionChange,
  initialView = 'board',
  rightAreaExpanded = false,
  readOnly = false,
  remoteTargets,
  trackServerId,
  surfaceMap,
  selectedSurfaceId = null,
  onOpenSurface,
  onSurfaceMapChange,
  researchProfileLabel,
  nowMs
}: {
  busy: boolean;
  appServerMemory: AppServerMemorySummary | null;
  activeScope?: WorkspaceScopeVersion | null;
  workspaceRules?: readonly WorkspaceRule[];
  researchProfile?: ResearchProfile | null;
  researchKitId?: ResearchKitId;
  sessionHeatPreferences?: SessionHeatPreferences;
  researchSubjectName?: string;
  workspaceId?: string;
  workspacePath?: string;
  workspaceDirectories?: readonly string[];
  memoryBackend?: WorkspaceMemoryBackendId;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  workspaceName: string;
  runs: RunRow[];
  initialView?: WorkspaceDashboardView;
  rightAreaExpanded?: boolean;
  readOnly?: boolean;
  remoteTargets?: readonly { id: string; name: string; ready: boolean }[];
  trackServerId?: string;
  surfaceMap?: BealeWorkspaceSurfaceMap | null;
  selectedSurfaceId?: string | null;
  onOpenSurface?: (surface: MapSurface) => void;
  onSurfaceMapChange?: (map: BealeWorkspaceSurfaceMap | null) => void;
  researchProfileLabel?: string;
  onAddResource?: (asset: ScopeAssetInput) => Promise<void>;
  onRefreshResearchKit?: (input: ResearchKitRefreshInput) => Promise<ResearchKitRefreshResult>;
  onAddRule?: (text: string) => Promise<void>;
  onSaveConfiguration?: (configuration: WorkspaceConfigurationInput) => Promise<void>;
  onChangeWorkspaceDirectories?: (directories: string[]) => Promise<void>;
  onChangeMemoryBackend?: (memoryBackend: WorkspaceMemoryBackendId) => Promise<void>;
  onChangeResearchSubject?: (researchSubjectName: string) => Promise<void>;
  onRemoveWorkspace?: () => Promise<void>;
  onOpenSession?: (runId: string) => void;
  onStartTrack?: () => void;
  onTrackResumed?: (track: BealeWorkspaceResearchTrack) => void;
  onOpenClaim?: (claimId: string) => void;
  openClaimId?: string | null;
  onTransitionClaim?: (claim: AppServerFindingSummary, targetMaturity: ClaimBoardMaturity) => Promise<void>;
  onOpenMemory?: (nodeId: string) => void;
  onOpenRunbook?: (runbookId: string) => void;
  onOpenReport?: (report: AppServerReportSummary) => void;
  onActiveViewChange?: (viewName: string) => void;
  onViewSelectionChange?: (view: WorkspaceDashboardView) => void;
  nowMs?: number;
}): JSX.Element {
  const [activeView, setActiveView] = useState<WorkspaceTopLevelView>(() => workspaceTopLevelView(initialView));
  const [addResourceKind, setAddResourceKind] = useState<ScopeAssetKind | null>(null);
  const [activeSurfaceKind, setActiveSurfaceKind] = useState<ScopeAssetKind | null>(null);
  const [surfaceKindPickerOpen, setSurfaceKindPickerOpen] = useState(false);
  const surfaceKindPickerRef = useRef<HTMLDivElement>(null);
  const [activeCampaignView, setActiveCampaignView] = useState<WorkspaceCampaignView>(() => workspaceCampaignView(initialView));
  const [focusedBoardMaturity, setFocusedBoardMaturity] = useState<ClaimBoardMaturity>('lead');
  const [clockNowMs, setClockNowMs] = useState(() => Date.now());
  const researchKit = researchKitDefinition(researchKitId);
  const dashboardViews = researchKit.refresh && !readOnly
    ? WORKSPACE_DASHBOARD_VIEWS
    : WORKSPACE_DASHBOARD_VIEWS.filter((view) => view !== 'kit');
  useEffect(() => {
    if (nowMs !== undefined) return undefined;
    const timer = window.setInterval(() => setClockNowMs(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, [nowMs]);
  useEffect(() => {
    onActiveViewChange?.(activeView === 'campaign'
      ? `Campaign ${workspaceCampaignViewLabel(activeCampaignView)}`
      : workspaceDashboardViewLabel(activeView, researchKitId));
  }, [activeCampaignView, activeView, onActiveViewChange, researchKitId]);
  useEffect(() => {
    onViewSelectionChange?.(activeView === 'campaign' ? activeCampaignView : activeView);
  }, [activeCampaignView, activeView, onViewSelectionChange]);
  const timelineNowMs = nowMs ?? clockNowMs;
  const memoryTypes = researchProfile?.memory.types ?? [];
  const campaignActive = activeView === 'campaign';
  const workspaceId = workspaceIdInput || appServerMemory?.contextWorkspaceId || '';
  const [localSurfaceMap, setLocalSurfaceMap] = useState<BealeWorkspaceSurfaceMap | null>(null);
  const [surfaceMapError, setSurfaceMapError] = useState<string | null>(null);
  useEffect(() => {
    if (activeView !== 'surfaces' || surfaceMap !== undefined || !workspaceId) return undefined;
    let active = true;
    const refresh = (): void => {
      void window.beale.getWorkspaceSurfaceMap(workspaceId)
        .then((value) => { if (active) { setLocalSurfaceMap(value); setSurfaceMapError(null); } })
        .catch((caught: unknown) => { if (active) setSurfaceMapError(errorMessage(caught)); });
    };
    setLocalSurfaceMap(null);
    setSurfaceMapError(null);
    refresh();
    const timer = window.setInterval(refresh, 5_000);
    return () => { active = false; window.clearInterval(timer); };
  }, [activeView, surfaceMap, workspaceId]);
  const visibleSurfaceMap = surfaceMap === undefined ? localSurfaceMap : surfaceMap;
  useEffect(() => {
    if (!surfaceKindPickerOpen) return undefined;
    const close = (event: PointerEvent): void => {
      if (!surfaceKindPickerRef.current?.contains(event.target as Node)) setSurfaceKindPickerOpen(false);
    };
    const escape = (event: KeyboardEvent): void => { if (event.key === 'Escape') setSurfaceKindPickerOpen(false); };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', escape);
    return () => { window.removeEventListener('pointerdown', close); window.removeEventListener('keydown', escape); };
  }, [surfaceKindPickerOpen]);
  useEffect(() => {
    if (activeView === 'surfaces') onSurfaceMapChange?.(visibleSurfaceMap);
  }, [activeView, onSurfaceMapChange, visibleSurfaceMap]);
  const workspaceMemoryNodes = useMemo(
    () => campaignActive && activeCampaignView === 'memory'
      ? (appServerMemory?.nodes ?? [])
          .filter((node) => workspaceId !== null && node.workspaces.some((workspace) => workspace.id === workspaceId))
          .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
      : [],
    [activeCampaignView, campaignActive, appServerMemory?.nodes, workspaceId]
  );
  const workspaceRunbooks = useMemo(
    () => campaignActive && activeCampaignView === 'runbooks'
      ? (appServerMemory?.runbooks ?? [])
          .filter((runbook) => workspaceId !== null && runbook.workspaceId === workspaceId)
          .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
      : [],
    [activeCampaignView, campaignActive, appServerMemory?.runbooks, workspaceId]
  );
  const workspaceReports = useMemo(
    () => campaignActive && activeCampaignView === 'reports'
      ? (appServerMemory?.reports ?? [])
          .filter((report) => report.workspaceId === workspaceId)
          .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
      : [],
    [activeCampaignView, campaignActive, appServerMemory?.reports, workspaceId]
  );

  return (
    <main className={`workspace-dashboard wide-content-container${campaignActive ? ' campaign-active' : ''}${activeView === 'surfaces' ? ' surfaces-active' : ''}`} aria-label="Workspace dashboard">
      <div className="workspace-dashboard-tabs research-side-view-tabs pill-view-tabs" role="tablist" aria-label="Workspace dashboard views">
        {dashboardViews.map((view) => {
          const selected = activeView === view;
          const ViewIcon = WORKSPACE_DASHBOARD_VIEW_ICONS[view];
          return (
            <div className={`research-side-view-tab provider-settings-tab workspace-dashboard-tab ${view === 'kit' ? 'workspace-dashboard-kit-tab ' : ''}${selected ? 'active' : ''}`.trim()} key={view}>
              <button
                aria-controls={view === 'campaign' ? 'workspace-dashboard-campaign-subviews' : `workspace-dashboard-${view}-panel`}
                aria-selected={selected}
                className="research-side-view-tab-activate"
                onClick={() => setActiveView(view)}
                role="tab"
                type="button"
              >
                <ViewIcon aria-hidden="true" className="workspace-dashboard-tab-icon" size={14} />
                <span>{workspaceDashboardViewLabel(view, researchKitId)}</span>
              </button>
            </div>
          );
        })}
      </div>

      {campaignActive ? (
        <div className="workspace-campaign-subview-navigation" id="workspace-dashboard-campaign-subviews" role="tabpanel">
          <div className="workspace-campaign-subview-tabs research-side-view-tabs pill-view-tabs" role="tablist" aria-label="Campaign views">
            {WORKSPACE_CAMPAIGN_VIEWS.map((view) => {
              const selected = activeCampaignView === view;
              const ViewIcon = WORKSPACE_CAMPAIGN_VIEW_ICONS[view];
              return (
                <div className={`research-side-view-tab provider-settings-tab workspace-campaign-subview-tab ${selected ? 'active' : ''}`.trim()} key={view}>
                  <button
                    aria-controls={`workspace-dashboard-campaign-${view}-panel`}
                    aria-selected={selected}
                    className="research-side-view-tab-activate"
                    onClick={() => setActiveCampaignView(view)}
                    role="tab"
                    type="button"
                  >
                    <ViewIcon aria-hidden="true" className="workspace-dashboard-tab-icon" size={14} />
                    <span>{workspaceCampaignViewLabel(view)}</span>
                  </button>
                </div>
              );
            })}
          </div>
          {workspaceId ? <WorkspaceResearchTrack workspaceId={workspaceId} serverId={trackServerId}
            enabled={memoryBackend !== 'disabled'} onOpenSession={onOpenSession} onStartTrack={onStartTrack}
            onTrackResumed={onTrackResumed} /> : null}
        </div>
      ) : null}

      {activeView === 'surfaces' ? <div className="workspace-surface-subview-navigation" ref={surfaceKindPickerRef}>
        <div className="workspace-surface-subview-tabs research-side-view-tabs pill-view-tabs" role="tablist" aria-label="Surface resource types">
          <div className={`research-side-view-tab workspace-resource-tab${activeSurfaceKind === null ? ' active' : ''}`}>
            <button className="research-side-view-tab-activate" type="button" role="tab" aria-selected={activeSurfaceKind === null}
              onClick={() => setActiveSurfaceKind(null)}><Network aria-hidden="true" size={14} /><span>All Surfaces</span></button>
            {!readOnly ? <button className="research-side-view-tab-close workspace-resource-tab-add" type="button"
              aria-label="Add resource" title="Add resource" aria-haspopup="menu" aria-expanded={surfaceKindPickerOpen}
              onClick={() => setSurfaceKindPickerOpen((open) => !open)}><Plus aria-hidden="true" size={14} /></button> : null}
          </div>
          {WORKSPACE_ASSET_KINDS.map((kind) => <div className={`research-side-view-tab workspace-resource-tab${activeSurfaceKind === kind ? ' active' : ''}`} key={kind}>
            <button className="research-side-view-tab-activate" type="button" role="tab" aria-selected={activeSurfaceKind === kind}
              onClick={() => setActiveSurfaceKind(kind)}><WorkspaceAssetIcon kind={kind} size={14} /><span>{workspaceAssetKindLabel(kind)}</span></button>
            {!readOnly ? <button className="research-side-view-tab-close workspace-resource-tab-add" type="button"
              aria-label={`Add ${workspaceAssetKindLabel(kind).toLowerCase()}`} title={`Add ${workspaceAssetKindLabel(kind).toLowerCase()}`}
              onClick={() => setAddResourceKind(kind)}><Plus aria-hidden="true" size={14} /></button> : null}
          </div>)}
        </div>
        {surfaceKindPickerOpen ? <div className="research-side-view-picker-menu workspace-resource-kind-menu workspace-surface-add-menu" role="menu">
          {WORKSPACE_ASSET_KINDS.map((kind) => <button type="button" role="menuitem" key={kind}
            onClick={() => { setSurfaceKindPickerOpen(false); setAddResourceKind(kind); }}>
            <WorkspaceAssetIcon kind={kind} /><span>{workspaceAssetKindLabel(kind)}</span>
          </button>)}
        </div> : null}
      </div> : null}

      {activeView === 'overview' ? <WorkspaceOverviewPanel
        activeScope={activeScope}
        busy={busy}
        hidden={false}
        onSave={onSaveConfiguration}
        onChangeDirectories={onChangeWorkspaceDirectories}
        onChangeMemoryBackend={onChangeMemoryBackend}
        onChangeResearchSubject={onChangeResearchSubject}
        onRemoveWorkspace={onRemoveWorkspace}
        memoryBackend={memoryBackend}
        memoryBackendLocked={runs.some(({ run }) => isLiveResearchRunStatus(run.status) || run.status === 'paused')}
        researchSubjectLocked={runs.some(({ run }) => isLiveResearchRunStatus(run.status))}
        researchProfile={researchProfile}
        researchProfileLabel={researchProfileLabel}
        readOnly={readOnly}
        researchKitId={researchKitId}
        researchSubjectName={researchSubjectName}
        workspaceName={workspaceName}
        workspaceId={workspaceId}
        workspacePath={workspacePath}
        workspaceDirectories={workspaceDirectories ?? (workspacePath ? [workspacePath] : [])}
      /> : null}

      {activeView === 'targets' ? <WorkspaceTargetsPanel workspaceId={workspaceId} workspaceName={activeScope?.workspaceName || workspaceName} disabled={busy} remoteTargets={remoteTargets} /> : null}

      {activeView === 'surfaces' ? <WorkspaceSurfaceMap
        map={visibleSurfaceMap}
        resourceKind={activeSurfaceKind}
        workspaceName={activeScope?.workspaceName || workspaceName}
        loading={surfaceMap === undefined && localSurfaceMap === null && !surfaceMapError}
        error={surfaceMapError}
        selectedSurfaceId={selectedSurfaceId}
        onSelectSurface={onOpenSurface}
      /> : null}

      {activeView === 'surfaces' && addResourceKind ? <WorkspaceResourceDialog
        initialAsset={null}
        kind={addResourceKind}
        onClose={() => setAddResourceKind(null)}
        onSubmit={onAddResource}
      /> : null}

      {campaignActive && activeCampaignView === 'board' ? <CampaignBoardView
        compact={rightAreaExpanded}
        focusedMaturity={focusedBoardMaturity}
        memory={appServerMemory}
        openClaimId={openClaimId}
        onFocusedMaturityChange={setFocusedBoardMaturity}
        providerModelCatalog={providerModelCatalog}
        workspaceName={activeScope?.workspaceName || workspaceName}
        onOpenClaim={onOpenClaim}
        onTransitionClaim={onTransitionClaim}
      /> : null}

      {activeView === 'kit' && researchKit.refresh ? <WorkspaceResearchKitPanel
        activeScope={activeScope}
        busy={busy}
        onRefresh={onRefreshResearchKit}
        researchKitId={researchKitId}
      /> : null}

      {activeView === 'rules' ? <WorkspaceRulesPanel
        busy={busy}
        onAddRule={onAddRule}
        readOnly={readOnly}
        rules={workspaceRules}
        workspaceName={activeScope?.workspaceName || workspaceName}
      /> : null}

      {campaignActive && activeCampaignView === 'memory' ? <WorkspaceMemoryPanel
        enabled={memoryBackend !== 'disabled'}
        hidden={false}
        loading={appServerMemory === null || appServerMemory.loading === true}
        memoryTypes={memoryTypes}
        nowMs={timelineNowMs}
        nodes={workspaceMemoryNodes}
        onOpen={onOpenMemory}
        profileId={researchProfile?.id}
        sessionHeatPreferences={sessionHeatPreferences}
        workspaceName={activeScope?.workspaceName || workspaceName}
      /> : null}

      {campaignActive && activeCampaignView === 'runbooks' ? <WorkspaceRunbooksPanel
        hidden={false}
        loading={appServerMemory === null || appServerMemory.loading === true}
        nowMs={timelineNowMs}
        runbooks={workspaceRunbooks}
        onOpen={onOpenRunbook}
        workspaceName={activeScope?.workspaceName || workspaceName}
      /> : null}

      {campaignActive && activeCampaignView === 'reports' ? <WorkspaceReportsPanel
        loading={appServerMemory === null || appServerMemory.loading === true}
        nowMs={timelineNowMs}
        reports={workspaceReports}
        onOpen={onOpenReport}
        workspaceName={activeScope?.workspaceName || workspaceName}
      /> : null}

    </main>
  );
}

function workspaceDashboardViewLabel(view: WorkspaceDashboardView, researchKitId: ResearchKitId): string {
  if (view === 'overview') return 'Settings';
  if (view === 'kit') return researchKitLabel(researchKitId);
  return view.charAt(0).toUpperCase() + view.slice(1);
}

function workspaceTopLevelView(initialView: WorkspaceDashboardView): WorkspaceTopLevelView {
  return initialView === 'resources' ? 'surfaces' : initialView === 'activity' || initialView === 'board' || initialView === 'memory' || initialView === 'runbooks' || initialView === 'reports'
    ? 'campaign'
    : initialView;
}

function workspaceCampaignView(initialView: WorkspaceDashboardView): WorkspaceCampaignView {
  return initialView === 'board' || initialView === 'memory' || initialView === 'runbooks' || initialView === 'reports'
    ? initialView
    : 'board';
}

function workspaceCampaignViewLabel(view: WorkspaceCampaignView): string {
  if (view === 'memory') return 'Memories';
  if (view === 'board') return 'Findings';
  return view.charAt(0).toUpperCase() + view.slice(1);
}

export function hackerOneProgramIdentifier(scope: WorkspaceScopeVersion | null): string {
  if (!scope) return '';
  for (const asset of scope.assets) {
    const handle = asset.attributes?.hackerOneHandle;
    if (typeof handle === 'string' && handle.trim()) return handle.trim();
  }
  return '';
}

export function bugcrowdProgramIdentifier(scope: WorkspaceScopeVersion | null): string {
  if (!scope) return '';
  for (const asset of scope.assets) {
    const slug = asset.attributes?.bugcrowdSlug;
    if (typeof slug === 'string' && slug.trim()) return slug.trim();
  }
  return '';
}

export function WorkspaceResearchKitPanel({
  activeScope,
  busy,
  onRefresh,
  researchKitId
}: {
  activeScope: WorkspaceScopeVersion | null;
  busy: boolean;
  onRefresh: (input: ResearchKitRefreshInput) => Promise<ResearchKitRefreshResult>;
  researchKitId: ResearchKitId;
}): JSX.Element {
  const kit = researchKitDefinition(researchKitId);
  const refresh = kit.refresh;
  const catalog = kit.resourceCatalog;
  const initialSource = researchKitId === 'hackerone' ? hackerOneProgramIdentifier(activeScope)
    : researchKitId === 'bugcrowd' ? bugcrowdProgramIdentifier(activeScope)
      : (refresh?.fixedSource ?? '');
  const [sourceIdentifier, setSourceIdentifier] = useState(initialSource);
  const [selectedResourceKeys, setSelectedResourceKeys] = useState<string[]>(() => activeScope?.assets
    .filter((asset) => asset.attributes?.researchKitId === researchKitId || asset.attributes?.source === catalog?.resourceSource)
    .map(researchKitResourceKey)
    .filter((key) => catalog?.resources.some((asset) => researchKitResourceKey(asset) === key)) ?? []);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ResearchKitRefreshResult | null>(null);
  useEffect(() => setSourceIdentifier(initialSource), [initialSource]);
  useEffect(() => setSelectedResourceKeys(activeScope?.assets
    .filter((asset) => asset.attributes?.researchKitId === researchKitId || asset.attributes?.source === catalog?.resourceSource)
    .map(researchKitResourceKey)
    .filter((key) => catalog?.resources.some((asset) => researchKitResourceKey(asset) === key)) ?? []), [activeScope, catalog?.resourceSource, researchKitId]);
  if (!refresh) throw new Error(`Research Kit ${researchKitId} does not define refresh behavior.`);
  const editableSource = Boolean(refresh.sourceIdentifierPlaceholder);
  const submit = async (event: FormEvent): Promise<void> => {
    event.preventDefault();
    if (editableSource && !sourceIdentifier.trim()) return;
    setSubmitting(true);
    setError(null);
    setResult(null);
    try {
      setResult(await onRefresh({
        ...(editableSource ? { sourceIdentifier: sourceIdentifier.trim() } : {}),
        ...(catalog ? { selectedResourceKeys } : {})
      }));
    } catch (caught: unknown) {
      setError(errorMessage(caught));
    } finally {
      setSubmitting(false);
    }
  };
  const importLabels = refresh.imports.map((kind) => kind === 'guidance' ? 'workspace guidance' : kind).join(', ');
  return (
    <section
      aria-label={`${kit.label} Research Kit`}
      className="workspace-dashboard-panel workspace-research-kit-view"
      id="workspace-dashboard-kit-panel"
      role="tabpanel"
    >
      <form className="settings-form workspace-research-kit-form" onSubmit={(event) => void submit(event)}>
        <header className="settings-form-heading">
          <h2>{kit.label} Research Kit</h2>
          <p>{kit.description}</p>
        </header>
        <div className="settings-form-squircle">
          <div className="settings-form-control-list">
            <label className="settings-form-control-row workspace-research-kit-source">
              <span className="settings-form-control-copy">
                <strong>{refresh.sourceLabel}</strong>
                <small>{refresh.sourceDescription}</small>
              </span>
              <input
                aria-label={refresh.sourceLabel}
                disabled={busy || submitting || !editableSource}
                onChange={(event) => setSourceIdentifier(event.target.value)}
                placeholder={refresh.sourceIdentifierPlaceholder}
                value={sourceIdentifier}
              />
            </label>
            <div className="settings-form-control-row workspace-research-kit-refresh">
              <span className="settings-form-control-copy">
                <strong>{catalog ? 'Apply Catalog & Refresh' : 'Refresh Imports'}</strong>
                <small>{catalog ? 'Apply selected catalog resources and refresh bundled rules and guidance. Unselected kit resources are removed; manual resources and checkout metadata are preserved.' : `Refresh ${importLabels}. Manually added resources and cloned directories are preserved.`}</small>
              </span>
              <button className="secondary-button" disabled={busy || submitting || (editableSource && !sourceIdentifier.trim())} type="submit">
                <RefreshCw aria-hidden="true" className={submitting ? 'is-spinning' : ''} size={14} />
                {submitting ? 'Refreshing…' : catalog ? 'Apply & Refresh' : 'Refresh'}
              </button>
            </div>
          </div>
          {catalog ? (
            <div className="workspace-research-kit-catalog">
              <p>Select resources observed on the Windows test guest. These entries do not establish installation, servicing eligibility, or an award category.</p>
              {['Windows apps', 'Windows services', 'Attack-scenario sandboxes', 'Shipped open source'].map((group) => {
                const resources = catalog.resources.filter((asset) => asset.attributes?.catalogGroup === group);
                if (!resources.length) return null;
                return <details key={group} open={group === 'Attack-scenario sandboxes'}>
                  <summary>{group} · {resources.filter((asset) => selectedResourceKeys.includes(researchKitResourceKey(asset))).length} selected</summary>
                  <div className="workspace-surface-list">
                    {resources.map((asset) => {
                      const key = researchKitResourceKey(asset);
                      const selected = selectedResourceKeys.includes(key);
                      return <label className="workspace-surface-item workspace-creation-candidate" key={key}>
                        <input aria-label={`Include ${String(asset.attributes?.displayName ?? asset.value)}`} checked={selected} disabled={busy || submitting} onChange={(event) => setSelectedResourceKeys((current) => event.target.checked ? [...current, key] : current.filter((value) => value !== key))} type="checkbox" />
                        <span className="workspace-surface-item-icon" aria-hidden="true"><WorkspaceAssetIcon kind={asset.kind} /></span>
                        <span className="workspace-surface-item-main"><strong>{String(asset.attributes?.displayName ?? asset.value)}</strong><small title={asset.value}>{asset.value}</small>{asset.attributes?.catalogNote ? <span className="workspace-surface-item-meta">{String(asset.attributes.catalogNote)}</span> : null}</span>
                      </label>;
                    })}
                  </div>
                </details>;
              })}
            </div>
          ) : null}
          {error ? <p className="workspace-research-kit-status is-error" role="alert">{error}</p> : null}
          {result ? <p className="workspace-research-kit-status" role="status">
            Refreshed {result.resourcesRefreshed} resources and {result.rulesRefreshed} rules{result.guidanceRefreshed ? ', plus workspace guidance' : ''}.
          </p> : null}
        </div>
      </form>
    </section>
  );
}

function WorkspaceRulesPanel({
  busy,
  onAddRule,
  readOnly,
  rules,
  workspaceName
}: {
  busy: boolean;
  onAddRule: (text: string) => Promise<void>;
  readOnly: boolean;
  rules: readonly WorkspaceRule[];
  workspaceName: string;
}): JSX.Element {
  const [draft, setDraft] = useState('');
  const [query, setQuery] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const filteredRules = useMemo(() => rules.filter((rule) => workspaceCatalogMatchesQuery(query, [rule.text, rule.id])), [rules, query]);
  const submit = async (event: FormEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    const text = draft.trim();
    if (!text || busy || submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      await onAddRule(text);
      setDraft('');
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setSubmitting(false);
    }
  };
  return (
    <section
      aria-label={`${workspaceName} rules`}
      className="workspace-dashboard-panel workspace-rules"
      id="workspace-dashboard-rules-panel"
      role="tabpanel"
    >
      <div className="workspace-rules-layout settings-form">
        <header className="settings-form-heading">
          <div className="workspace-catalog-title-row"><h2>{workspaceName} Rules</h2><WorkspaceTitleSearch label="rules" query={query} onChange={setQuery} /></div>
          <p>{readOnly ? 'Operating constraints for research in this workspace.' : 'Append concise operating constraints for every research session in this workspace.'}</p>
        </header>
        <div className={`settings-form-squircle workspace-rules-surface${readOnly ? ' is-read-only' : ''}`}>
          {!readOnly ? <form className="workspace-rule-composer" onSubmit={(event) => void submit(event)}>
            <input
              aria-label="New workspace rule"
              disabled={busy || submitting}
              maxLength={2000}
              placeholder="Add a rule"
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
            />
            <button disabled={busy || submitting || !draft.trim()} type="submit">
              <Plus aria-hidden="true" size={14} />
              Add Rule
            </button>
          </form> : null}
          {error ? <p className="workspace-rules-error" role="alert">{error}</p> : null}
          {filteredRules.length > 0 ? (
            <ol aria-label="Workspace rules" className="workspace-rule-list">
              {filteredRules.map((rule) => <li key={rule.id}>{rule.text}</li>)}
            </ol>
          ) : <p className="workspace-rules-empty">{query.trim() ? 'No rules match this search.' : 'No workspace rules recorded.'}</p>}
        </div>
      </div>
    </section>
  );
}

function WorkspaceOverviewPanel({
  activeScope,
  busy,
  hidden,
  onChangeDirectories,
  onChangeMemoryBackend,
  onChangeResearchSubject,
  onRemoveWorkspace,
  onSave,
  memoryBackend,
  memoryBackendLocked,
  researchProfile,
  researchProfileLabel,
  readOnly,
  researchKitId,
  researchSubjectName,
  researchSubjectLocked,
  workspaceName,
  workspaceId,
  workspacePath,
  workspaceDirectories
}: {
  activeScope: WorkspaceScopeVersion | null;
  busy: boolean;
  hidden: boolean;
  onChangeDirectories: (directories: string[]) => Promise<void>;
  onChangeMemoryBackend: (memoryBackend: WorkspaceMemoryBackendId) => Promise<void>;
  onChangeResearchSubject: (researchSubjectName: string) => Promise<void>;
  onRemoveWorkspace: () => Promise<void>;
  onSave: (configuration: WorkspaceConfigurationInput) => Promise<void>;
  memoryBackend: WorkspaceMemoryBackendId;
  memoryBackendLocked: boolean;
  researchProfile: ResearchProfile | null;
  researchProfileLabel?: string;
  readOnly: boolean;
  researchKitId: ResearchKitId;
  researchSubjectName: string;
  researchSubjectLocked: boolean;
  workspaceName: string;
  workspaceId: string;
  workspacePath: string;
  workspaceDirectories: readonly string[];
}): JSX.Element {
  const resolvedWorkspaceName = activeScope?.workspaceName || workspaceName;
  const resolvedDescription = activeScope?.descriptionMarkdown ?? '';
  const [workspaceNameDraft, setWorkspaceNameDraft] = useState(resolvedWorkspaceName);
  const [researchSubjectDraft, setResearchSubjectDraft] = useState(researchSubjectName);
  const [descriptionDraft, setDescriptionDraft] = useState(resolvedDescription);
  const [guidanceEditing, setGuidanceEditing] = useState(false);
  const [guidanceHeight, setGuidanceHeight] = useState(150);
  const [saving, setSaving] = useState(false);
  const [memorySaving, setMemorySaving] = useState(false);
  const [researchSubjectSaving, setResearchSubjectSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const guidancePreviewRef = useRef<HTMLDivElement>(null);
  const guidanceEditorRef = useRef<HTMLTextAreaElement>(null);
  const resolvedConfigurationRef = useRef({
    workspaceName: resolvedWorkspaceName,
    descriptionMarkdown: resolvedDescription
  });
  const saveQueueRef = useRef<Promise<void>>(Promise.resolve());
  const pendingSaveCountRef = useRef(0);
  const lastQueuedConfigurationRef = useRef<string | null>(null);
  useEffect(() => {
    const previous = resolvedConfigurationRef.current;
    setWorkspaceNameDraft((current) => current === previous.workspaceName ? resolvedWorkspaceName : current);
    setDescriptionDraft((current) => current === previous.descriptionMarkdown ? resolvedDescription : current);
    setGuidanceEditing(false);
    setGuidanceHeight(150);
    resolvedConfigurationRef.current = {
      workspaceName: resolvedWorkspaceName,
      descriptionMarkdown: resolvedDescription
    };
    if (lastQueuedConfigurationRef.current === JSON.stringify(resolvedConfigurationRef.current)) {
      lastQueuedConfigurationRef.current = null;
    }
    setSaveError(null);
  }, [activeScope?.id, resolvedDescription, resolvedWorkspaceName]);
  useEffect(() => {
    setResearchSubjectDraft(researchSubjectName);
  }, [researchSubjectName]);
  const saveInPlace = (): void => {
    const configuration = {
      workspaceName: workspaceNameDraft,
      descriptionMarkdown: descriptionDraft
    };
    const configurationKey = JSON.stringify(configuration);
    const dirty = configuration.workspaceName !== resolvedWorkspaceName
      || configuration.descriptionMarkdown !== resolvedDescription;
    if (!configuration.workspaceName.trim()) {
      setSaveError('Workspace name is required.');
      return;
    }
    if (!dirty || busy || configurationKey === lastQueuedConfigurationRef.current) return;
    lastQueuedConfigurationRef.current = configurationKey;
    pendingSaveCountRef.current += 1;
    setSaving(true);
    setSaveError(null);
    saveQueueRef.current = saveQueueRef.current
      .catch(() => undefined)
      .then(() => onSave(configuration))
      .catch((caught: unknown) => {
        lastQueuedConfigurationRef.current = null;
        setSaveError(errorMessage(caught));
      })
      .finally(() => {
        pendingSaveCountRef.current -= 1;
        if (pendingSaveCountRef.current === 0) setSaving(false);
      });
  };
  const showGuidanceEditor = (): void => {
    if (busy) return;
    const previewHeight = guidancePreviewRef.current?.getBoundingClientRect().height;
    if (previewHeight && previewHeight >= 150) setGuidanceHeight(previewHeight);
    setGuidanceEditing(true);
  };
  const changeMemoryBackend = async (nextMemoryBackend: WorkspaceMemoryBackendId): Promise<void> => {
    if (busy || memoryBackendLocked || memorySaving || nextMemoryBackend === memoryBackend) return;
    setMemorySaving(true);
    setSaveError(null);
    try {
      await onChangeMemoryBackend(nextMemoryBackend);
    } catch (caught) {
      setSaveError(errorMessage(caught));
    } finally {
      setMemorySaving(false);
    }
  };
  const saveResearchSubject = async (): Promise<void> => {
    const name = researchSubjectDraft.trim().replace(/\s+/g, ' ');
    if (!name) {
      setSaveError('Research Subject is required.');
      return;
    }
    if (busy || researchSubjectLocked || researchSubjectSaving || name === researchSubjectName) return;
    setResearchSubjectSaving(true);
    setSaveError(null);
    try {
      await onChangeResearchSubject(name);
    } catch (caught) {
      setSaveError(errorMessage(caught));
    } finally {
      setResearchSubjectSaving(false);
    }
  };
  const showGuidancePreview = (): void => {
    const editorHeight = guidanceEditorRef.current?.getBoundingClientRect().height;
    if (editorHeight && editorHeight >= 150) setGuidanceHeight(editorHeight);
    setGuidanceEditing(false);
  };
  return (
    <section
      aria-label="Workspace settings"
      className="workspace-dashboard-panel workspace-overview"
      hidden={hidden}
      id="workspace-dashboard-overview-panel"
      role="tabpanel"
    >
      <div className="workspace-overview-layout settings-form">
        <header className="settings-form-heading workspace-overview-heading">
          <h2 id="workspace-overview-heading">{workspaceNameDraft.trim() || resolvedWorkspaceName} Settings</h2>
          <p>Review the workspace context and authorized research boundary.</p>
        </header>
        <div className="workspace-overview-form">
          <div className="settings-form-squircle" aria-labelledby="workspace-overview-heading">
            <div className="settings-form-control-list">
              <label className="settings-form-control-row workspace-overview-control-row">
                <span className="settings-form-control-copy">
                  <strong>Research Profile</strong>
                  <small>The research profile that defines this workspace.</small>
                </span>
                <input
                  aria-label="Research Profile"
                  className="workspace-overview-input"
                  disabled
                  value={researchProfileLabel ?? workspaceResearchProfileLabel(researchProfile)}
                />
              </label>
              <label className="settings-form-control-row workspace-overview-control-row">
                <span className="settings-form-control-copy">
                  <strong>Research Kit</strong>
                  <small>The fixed resource, scope, and rule acquisition kit selected when this workspace was created.</small>
                </span>
                <input
                  aria-label="Research Kit"
                  className="workspace-overview-input"
                  disabled
                  value={researchKitLabel(researchKitId)}
                />
              </label>
              <label className="settings-form-control-row workspace-overview-control-row">
                <span className="settings-form-control-copy">
                  <strong>Memory</strong>
                  <small>Disabling memory retains existing data and removes recall, claim, and campaign tools from new sessions.</small>
                </span>
                <select
                  aria-label="Memory"
                  className="workspace-overview-input"
                  disabled={readOnly || busy || memoryBackendLocked || memorySaving}
                  title={memoryBackendLocked ? 'Memory cannot be changed while a research session is queued, active, or paused' : undefined}
                  value={memoryBackend}
                  onChange={(event) => void changeMemoryBackend(event.target.value as WorkspaceMemoryBackendId)}
                >
                  <option value="app-server">Enabled</option>
                  <option value="disabled">Disabled</option>
                </select>
              </label>
              <label className="settings-form-control-row workspace-overview-control-row">
                <span className="settings-form-control-copy">
                  <strong>Research Subject</strong>
                  <small>The research subject shared across related workspaces.</small>
                </span>
                <input
                  aria-label="Research Subject"
                  className="workspace-overview-input"
                  disabled={readOnly || busy || researchSubjectLocked || researchSubjectSaving}
                  maxLength={500}
                  required
                  title={researchSubjectLocked ? 'Research Subject cannot be changed while a research session is queued or active' : undefined}
                  value={researchSubjectDraft}
                  onChange={(event) => setResearchSubjectDraft(event.target.value)}
                  onBlur={() => void saveResearchSubject()}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') event.currentTarget.blur();
                  }}
                />
              </label>
              <label className="settings-form-control-row workspace-overview-control-row">
                <span className="settings-form-control-copy">
                  <strong>Workspace Name</strong>
                  <small>Choose the name shown throughout Beale.</small>
                </span>
                <input
                  aria-label="Workspace Name"
                  className="workspace-overview-input"
                  disabled={readOnly || busy}
                  required
                  value={workspaceNameDraft}
                  onChange={(event) => setWorkspaceNameDraft(event.target.value)}
                  onBlur={saveInPlace}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') event.currentTarget.blur();
                  }}
                />
              </label>
              {!readOnly ? <WorkspaceDirectoriesField
                directories={workspaceDirectories}
                disabled={busy}
                lockedDirectory={workspacePath}
                onAdd={async (selection) => {
                  const selectedPath = selection.path;
                  if (!selectedPath || workspaceDirectories.some((directory) => workspaceDirectoryKey(directory) === workspaceDirectoryKey(selectedPath))) return;
                  if (selection.knownWorkspace) {
                    throw new Error(`Directory already belongs to workspace ${selection.knownWorkspace.workspaceName}.`);
                  }
                  await onChangeDirectories([...workspaceDirectories, selectedPath]);
                }}
                onRemove={(directory) => onChangeDirectories(workspaceDirectories.filter((item) => workspaceDirectoryKey(item) !== workspaceDirectoryKey(directory)))}
              /> : null}
              <div className="settings-form-control-row workspace-overview-control-row workspace-overview-textarea-row">
                <div className="workspace-guidance-field-heading">
                  <span className="settings-form-control-copy">
                    <strong>Workspace Guidance</strong>
                    <small>{readOnly ? 'Instructions recorded by this app server.' : "Reads and writes the primary workspace's AGENTS.md instructions."}</small>
                  </span>
                  {guidanceEditing && !readOnly ? (
                    <button className="workspace-guidance-show-markdown" onClick={showGuidancePreview} type="button">
                      Show Markdown
                    </button>
                  ) : null}
                </div>
                {guidanceEditing && !readOnly ? (
                  <textarea
                    aria-label="Workspace Guidance"
                    autoFocus
                    className="workspace-guidance-editor"
                    disabled={busy}
                    ref={guidanceEditorRef}
                    rows={5}
                    style={{ height: guidanceHeight }}
                    value={descriptionDraft}
                    onChange={(event) => setDescriptionDraft(event.target.value)}
                    onBlur={saveInPlace}
                  />
                ) : (
                  <div
                    className="workspace-guidance-preview"
                    ref={guidancePreviewRef}
                    style={{ height: guidanceHeight }}
                  >
                    <div
                      aria-label="Workspace Guidance"
                      aria-disabled={busy || readOnly || undefined}
                      className="workspace-guidance-preview-content"
                      onClick={readOnly ? undefined : showGuidanceEditor}
                      onKeyDown={readOnly ? undefined : (event) => {
                        if (event.target !== event.currentTarget || (event.key !== 'Enter' && event.key !== ' ')) return;
                        event.preventDefault();
                        showGuidanceEditor();
                      }}
                      role={readOnly ? undefined : 'button'}
                      tabIndex={busy || readOnly ? -1 : 0}
                      title={readOnly ? undefined : 'Edit Workspace Guidance'}
                    >
                      {descriptionDraft.trim()
                        ? renderTraceProseText(descriptionDraft, 'agent_output')
                        : <p className="workspace-guidance-preview-empty">{readOnly ? 'No workspace guidance recorded.' : 'Click to add workspace guidance.'}</p>}
                    </div>
                  </div>
                )}
              </div>
              {!readOnly ? <WorkspaceRemovalForm busy={busy} onRemove={onRemoveWorkspace} workspaceName={resolvedWorkspaceName} /> : null}
            </div>
          </div>
          {saveError ? <p className="workspace-overview-error" role="alert">{saveError}</p> : null}
          {saving ? <span className="workspace-overview-saving" role="status">Saving…</span> : null}
        </div>
      </div>
    </section>
  );
}

function workspaceDirectoryKey(directory: string): string {
  return directory.replace(/[\\/]+$/u, '').toLowerCase();
}

function workspaceResearchProfileLabel(profile: ResearchProfile | null): string {
  if (!profile) return '';
  if (profile.id === 'security-research') return 'Security';
  return profile.name;
}

function WorkspaceActivityForm({
  activity,
  metric,
  search,
  showGrid = true,
  viewLabel,
  workspaceName
}: {
  activity: WorkspaceHeatmapActivity;
  metric: WorkspaceHeatmapMetric;
  search?: { query: string; onChange: (query: string) => void };
  showGrid?: boolean;
  viewLabel: string;
  workspaceName: string;
}): JSX.Element {
  const metricCopy = workspaceHeatmapMetricCopy(metric);
  return (
    <section className="settings-form workspace-activity-form" aria-label={`${workspaceName} ${viewLabel.toLowerCase()} yearly ${metricCopy.activityLabel}`}>
      <header className="settings-form-heading">
        {search ? <div className="workspace-catalog-title-row">
          <h2>{workspaceName} {viewLabel}</h2>
          <WorkspaceTitleSearch label={viewLabel.toLowerCase()} query={search.query} onChange={search.onChange} />
        </div> : <h2>{workspaceName} {viewLabel}</h2>}
        <p>{workspaceHeatmapValueLabel(activity.total, metric)} over the past year.</p>
      </header>
      {showGrid ? <div className="workspace-activity-grid-scroll">
        <div className="workspace-activity-grid" role="grid" aria-label={`Daily ${metricCopy.activityLabel} over the past year`}>
          {Array.from({ length: activity.leadingEmptyDays }, (_, index) => (
            <span aria-hidden="true" className="workspace-activity-cell is-empty" key={`empty-${index}`} />
          ))}
          {activity.days.map((day) => {
            const dateLabel = new Date(day.timestamp).toLocaleDateString(undefined, {
              month: 'short',
              day: 'numeric',
              year: 'numeric',
              timeZone: 'UTC'
            });
            const label = `${dateLabel}: ${workspaceHeatmapValueLabel(day.value, metric)}`;
            return (
              <span
                aria-label={label}
                className={`workspace-activity-cell heat-${day.heatLevel}`}
                data-activity-count={day.value}
                data-date={day.dateKey}
                data-metric={metric}
                key={day.dateKey}
                role="gridcell"
                title={label}
              />
            );
          })}
        </div>
      </div> : null}
    </section>
  );
}

type WorkspaceHeatmapMetric = 'tokens' | 'resources' | 'memories' | 'runbooks' | 'reports';

function workspaceHeatmapMetricCopy(metric: WorkspaceHeatmapMetric): { activityLabel: string } {
  switch (metric) {
    case 'tokens': return { activityLabel: 'token usage' };
    case 'resources': return { activityLabel: 'resource creation' };
    case 'memories': return { activityLabel: 'memory creation' };
    case 'runbooks': return { activityLabel: 'runbook creation' };
    case 'reports': return { activityLabel: 'report creation' };
  }
}

function workspaceHeatmapValueLabel(value: number, metric: WorkspaceHeatmapMetric): string {
  const formattedValue = value.toLocaleString();
  switch (metric) {
    case 'tokens': return `${formattedValue} ${value === 1 ? 'token' : 'tokens'} used`;
    case 'resources': return `${formattedValue} ${value === 1 ? 'resource' : 'resources'} created`;
    case 'memories': return `${formattedValue} ${value === 1 ? 'memory' : 'memories'} created`;
    case 'runbooks': return `${formattedValue} ${value === 1 ? 'runbook' : 'runbooks'} created`;
    case 'reports': return `${formattedValue} ${value === 1 ? 'report' : 'reports'} created`;
  }
}

export interface WorkspaceMemoryTypeGroup {
  type: string;
  nodes: AppServerMemorySummary['nodes'];
}

export function workspaceMemoryTypeGroups(
  nodes: AppServerMemorySummary['nodes'],
  memoryTypes: ResearchProfile['memory']['types'],
  profileId: string | null | undefined,
  sessionHeatPreferences: SessionHeatPreferences
): WorkspaceMemoryTypeGroup[] {
  return memoryTypeGroupsByHeat(nodes, memoryTypes, profileId, sessionHeatPreferences.heatOverrides);
}

function WorkspaceMemoryPanel({
  enabled,
  hidden,
  loading,
  memoryTypes,
  nowMs,
  nodes,
  onOpen,
  profileId,
  sessionHeatPreferences,
  workspaceName
}: {
  enabled: boolean;
  hidden: boolean;
  loading: boolean;
  memoryTypes: ResearchProfile['memory']['types'];
  nowMs: number;
  nodes: AppServerMemorySummary['nodes'];
  onOpen: (nodeId: string) => void;
  profileId?: string | null;
  sessionHeatPreferences: SessionHeatPreferences;
  workspaceName: string;
}): JSX.Element {
  const [query, setQuery] = useState('');
  const activity = useMemo(() => workspaceCreationActivity(nodes, nowMs), [nodes, nowMs]);
  const filteredNodes = useMemo(() => nodes.filter((node) => workspaceCatalogMatchesQuery(query,
    [node.id, node.title, node.summary, node.body, node.type, node.status, ...node.tags])), [nodes, query]);
  const groups = useMemo(
    () => workspaceMemoryTypeGroups(filteredNodes, memoryTypes, profileId, sessionHeatPreferences),
    [filteredNodes, memoryTypes, profileId, sessionHeatPreferences]
  );
  return (
    <section
      aria-label="Workspace memories"
      className="workspace-dashboard-panel workspace-catalog-view workspace-campaign-catalog-view workspace-memories-view"
      hidden={hidden}
      id="workspace-dashboard-campaign-memory-panel"
      role="tabpanel"
    >
      <WorkspaceActivityForm activity={activity} metric="memories" viewLabel="Memories" workspaceName={workspaceName}
        search={{ query, onChange: setQuery }} />
      <MainSideScrollRegion
        className="workspace-catalog-list workspace-memory-scroll"
        listClassName="memory-catalog-list workspace-memory-type-lists"
        updateKey={`${loading}:${query}:${filteredNodes.length}:${groups.length}`}
      >
        {groups.map((group) => (
          <WorkspaceMemoryTypeSection
            group={group}
            key={group.type}
            memoryTypes={memoryTypes}
            onOpen={onOpen}
          />
        ))}
        {filteredNodes.length === 0 ? (
          <p className="workspace-catalog-empty">{query.trim() ? 'No memories match this search.' : !enabled ? 'Memory is disabled for this workspace.' : loading ? 'Loading memory.' : 'No workspace memory yet.'}</p>
        ) : null}
      </MainSideScrollRegion>
    </section>
  );
}

function WorkspaceMemoryTypeSection({
  group,
  memoryTypes,
  onOpen
}: {
  group: WorkspaceMemoryTypeGroup;
  memoryTypes: ResearchProfile['memory']['types'];
  onOpen: (nodeId: string) => void;
}): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const visibleNodes = group.nodes.slice(0, 4);
  const overflowNodes = group.nodes.slice(4);
  const definition = memoryTypeDefinition(group.type, memoryTypes);
  const fallbackLabel = memoryTypeLabel(group.type, memoryTypes);
  const typeLabel = group.nodes.length === 1
    ? definition?.name ?? fallbackLabel
    : definition?.pluralName ?? workspaceMemoryTypePluralLabel(fallbackLabel);
  const renderNode = (node: AppServerMemorySummary['nodes'][number]): JSX.Element => (
    <MemoryCatalogItem
      key={node.id}
      node={node}
      selected={false}
      onOpen={() => onOpen(node.id)}
    />
  );
  return (
    <section className="workspace-memory-type-section" aria-label={`${group.nodes.length} ${typeLabel}`}>
      <h3 className="workspace-campaign-list-heading">{group.nodes.length.toLocaleString()} {typeLabel}</h3>
      <div className="workspace-memory-type-primary-items">
        {visibleNodes.map(renderNode)}
      </div>
      {overflowNodes.length > 0 ? (
        <>
          <div
            aria-hidden={!expanded}
            className={`workspace-memory-type-overflow ${expanded ? 'expanded' : ''}`.trim()}
            inert={expanded ? undefined : true}
          >
            <div>{overflowNodes.map(renderNode)}</div>
          </div>
          <button
            aria-expanded={expanded}
            className="session-memory-type-toggle workspace-memory-type-toggle"
            onClick={() => setExpanded((current) => !current)}
            type="button"
          >
            {expanded ? 'Show less' : `Show ${overflowNodes.length.toLocaleString()} more`}
          </button>
        </>
      ) : null}
    </section>
  );
}

function workspaceMemoryTypePluralLabel(label: string): string {
  if (label.startsWith('Unknown type (')) return label.replace('Unknown type (', 'Unknown types (');
  return label.endsWith('s') ? label : `${label}s`;
}

function WorkspaceRunbooksPanel({
  hidden,
  loading,
  nowMs,
  runbooks,
  onOpen,
  workspaceName
}: {
  hidden: boolean;
  loading: boolean;
  nowMs: number;
  runbooks: AppServerMemorySummary['runbooks'];
  onOpen: (runbookId: string) => void;
  workspaceName: string;
}): JSX.Element {
  const [query, setQuery] = useState('');
  const activity = useMemo(() => workspaceCreationActivity(runbooks, nowMs), [nowMs, runbooks]);
  const filteredRunbooks = useMemo(() => runbooks.filter((runbook) => workspaceCatalogMatchesQuery(query,
    [runbook.id, runbook.title, runbook.purpose])), [query, runbooks]);
  return (
    <section
      aria-label="Workspace runbooks"
      className="workspace-dashboard-panel workspace-catalog-view workspace-campaign-catalog-view workspace-runbooks-view"
      hidden={hidden}
      id="workspace-dashboard-campaign-runbooks-panel"
      role="tabpanel"
    >
      <WorkspaceActivityForm activity={activity} metric="runbooks" viewLabel="Runbooks" workspaceName={workspaceName}
        search={{ query, onChange: setQuery }} />
      <MainSideScrollRegion
        className="workspace-catalog-list workspace-runbook-scroll"
        listClassName="memory-catalog-list runbook-catalog-list workspace-runbook-list"
        updateKey={`${loading}:${query}:${filteredRunbooks.length}`}
      >
        {filteredRunbooks.length > 0 ? <WorkspaceRunbookSection runbooks={filteredRunbooks} onOpen={onOpen} /> : (
          <p className="workspace-catalog-empty">{query.trim() ? 'No runbooks match this search.' : loading ? 'Loading runbooks.' : 'No workspace runbooks yet.'}</p>
        )}
      </MainSideScrollRegion>
    </section>
  );
}

function WorkspaceRunbookSection({
  runbooks,
  onOpen
}: {
  runbooks: AppServerMemorySummary['runbooks'];
  onOpen: (runbookId: string) => void;
}): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const visibleRunbooks = runbooks.slice(0, 4);
  const overflowRunbooks = runbooks.slice(4);
  const label = runbooks.length === 1 ? 'Runbook' : 'Runbooks';
  const renderRunbook = (runbook: AppServerMemorySummary['runbooks'][number]): JSX.Element => (
    <RunbookCatalogItem
      key={runbook.id}
      runbook={runbook}
      selected={false}
      onOpen={() => onOpen(runbook.id)}
    />
  );
  return (
    <section className="workspace-memory-type-section" aria-label={`${runbooks.length} ${label}`}>
      <h3 className="workspace-campaign-list-heading">{runbooks.length.toLocaleString()} {label}</h3>
      <div className="workspace-memory-type-primary-items">
        {visibleRunbooks.map(renderRunbook)}
      </div>
      {overflowRunbooks.length > 0 ? (
        <>
          <div
            aria-hidden={!expanded}
            className={`workspace-memory-type-overflow ${expanded ? 'expanded' : ''}`.trim()}
            inert={expanded ? undefined : true}
          >
            <div>{overflowRunbooks.map(renderRunbook)}</div>
          </div>
          <button
            aria-expanded={expanded}
            className="session-memory-type-toggle workspace-memory-type-toggle"
            onClick={() => setExpanded((current) => !current)}
            type="button"
          >
            {expanded ? 'Show less' : `Show ${overflowRunbooks.length.toLocaleString()} more`}
          </button>
        </>
      ) : null}
    </section>
  );
}

function WorkspaceReportsPanel({
  loading,
  nowMs,
  reports,
  onOpen,
  workspaceName
}: {
  loading: boolean;
  nowMs: number;
  reports: AppServerReportSummary[];
  onOpen: (report: AppServerReportSummary) => void;
  workspaceName: string;
}): JSX.Element {
  const [query, setQuery] = useState('');
  const activity = useMemo(() => workspaceCreationActivity(reports, nowMs), [nowMs, reports]);
  const filteredReports = useMemo(() => reports.filter((report) => workspaceCatalogMatchesQuery(query,
    [report.id, report.title, report.summary, report.status, report.triageStatus])), [query, reports]);
  return (
    <section
      aria-label="Workspace reports"
      className="workspace-dashboard-panel workspace-catalog-view workspace-campaign-catalog-view workspace-reports-view"
      id="workspace-dashboard-campaign-reports-panel"
      role="tabpanel"
    >
      <WorkspaceActivityForm activity={activity} metric="reports" viewLabel="Reports" workspaceName={workspaceName}
        search={{ query, onChange: setQuery }} />
      <MainSideScrollRegion
        className="workspace-catalog-list workspace-report-scroll"
        listClassName="memory-catalog-list runbook-catalog-list report-catalog-list workspace-report-list"
        updateKey={`${loading}:${query}:${filteredReports.length}`}
      >
        {filteredReports.length > 0 ? <WorkspaceReportSection reports={filteredReports} nowMs={nowMs} onOpen={onOpen} /> : (
          <p className="workspace-catalog-empty">{query.trim() ? 'No reports match this search.' : loading ? 'Loading reports.' : 'No workspace reports yet.'}</p>
        )}
      </MainSideScrollRegion>
    </section>
  );
}

function WorkspaceReportSection({
  reports,
  nowMs,
  onOpen
}: {
  reports: AppServerReportSummary[];
  nowMs: number;
  onOpen: (report: AppServerReportSummary) => void;
}): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const visibleReports = reports.slice(0, 4);
  const overflowReports = reports.slice(4);
  const label = reports.length === 1 ? 'Report' : 'Reports';
  const renderReport = (report: AppServerReportSummary): JSX.Element => (
    <ReportCatalogItem
      key={report.id}
      report={report}
      nowMs={nowMs}
      selected={false}
      onOpen={() => onOpen(report)}
    />
  );
  return (
    <section className="workspace-memory-type-section" aria-label={`${reports.length} ${label}`}>
      <h3 className="workspace-campaign-list-heading">{reports.length.toLocaleString()} {label}</h3>
      <div className="workspace-memory-type-primary-items">{visibleReports.map(renderReport)}</div>
      {overflowReports.length > 0 ? (
        <>
          <div
            aria-hidden={!expanded}
            className={`workspace-memory-type-overflow ${expanded ? 'expanded' : ''}`.trim()}
            inert={expanded ? undefined : true}
          >
            <div>{overflowReports.map(renderReport)}</div>
          </div>
          <button
            aria-expanded={expanded}
            className="session-memory-type-toggle workspace-memory-type-toggle"
            onClick={() => setExpanded((current) => !current)}
            type="button"
          >
            {expanded ? 'Show less' : `Show ${overflowReports.length.toLocaleString()} more`}
          </button>
        </>
      ) : null}
    </section>
  );
}

function WorkspaceRemovalForm({
  busy,
  onRemove,
  workspaceName
}: {
  busy: boolean;
  onRemove: () => Promise<void>;
  workspaceName: string;
}): JSX.Element {
  const [confirmation, setConfirmation] = useState('');
  const [removing, setRemoving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const confirmed = workspaceRemovalConfirmationMatches(confirmation, workspaceName);
  const submit = async (event: FormEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    if (!confirmed || busy || removing) return;
    setRemoving(true);
    setError(null);
    try {
      await onRemove();
    } catch (caught) {
      setError(errorMessage(caught));
      setRemoving(false);
    }
  };
  return (
    <form className="settings-form-control-row workspace-overview-control-row workspace-removal-form" onSubmit={(event) => void submit(event)}>
      <span className="settings-form-control-copy">
        <strong>Remove Workspace</strong>
        <small>
          Unregisters this workspace from Beale only. Directories, .beale metadata, repository clones, scoped resources, and app-server memory remain on disk.
        </small>
      </span>
      <div className="workspace-removal-controls">
        <input
          aria-label={`Type ${workspaceName} to confirm workspace removal`}
          autoComplete="off"
          disabled={busy || removing}
          placeholder={`Type "${workspaceName}" to confirm`}
          spellCheck={false}
          value={confirmation}
          onChange={(event) => setConfirmation(event.target.value)}
        />
        <button
          className="workspace-removal-action"
          disabled={busy || removing || !confirmed}
          type="submit"
        >
          {removing ? 'Removing…' : 'Remove from Beale'}
        </button>
        {error ? <p className="workspace-removal-error" role="alert">{error}</p> : null}
      </div>
    </form>
  );
}

export function workspaceRemovalConfirmationMatches(confirmation: string, workspaceName: string): boolean {
  return normalizedWorkspaceRemovalName(confirmation) === normalizedWorkspaceRemovalName(workspaceName);
}

function normalizedWorkspaceRemovalName(value: string): string {
  return value
    .normalize('NFKC')
    .replace(/[\u200B-\u200D\u2060\uFEFF]/gu, '')
    .replace(/\s+/gu, ' ')
    .trim();
}

export interface WorkspaceResearchSurfaceItem {
  asset: ScopeAsset;
  assetIds: string[];
  label: string;
  repositoryCloned: boolean | null;
  repositoryCloneAssetId: string | null;
  repositoryLocalPath: string | null;
}

export function workspaceResearchSurfaceItems(
  assets: readonly ScopeAsset[],
  repositoryCheckouts?: Readonly<Record<string, string>>
): WorkspaceResearchSurfaceItem[] {
  const assetGroups = new Map<string, ScopeAsset[]>();
  for (const asset of assets) {
    const key = workspaceAssetGroupKey(asset);
    assetGroups.set(key, [...(assetGroups.get(key) ?? []), asset]);
  }
  return [...assetGroups.values()].map((groupAssets) => {
    const asset = preferredWorkspaceSurfaceAsset(groupAssets);
    const assetIds = groupAssets.map((candidate) => candidate.id);
    const repositorySourceAsset = asset.kind === 'repo'
      ? groupAssets.find((candidate) => repositoryNameFromUrl(candidate.value)) ?? null
      : null;
    const repositoryLocalAsset = asset.kind === 'repo'
      ? groupAssets.find((candidate) => !repositoryNameFromUrl(candidate.value) && repositoryIdentityFromPath(candidate.value)) ?? null
      : null;
    const clonedDirectory = asset.kind === 'repo'
      ? repositoryCheckouts
        ? groupAssets.map((candidate) => repositoryCheckouts[candidate.id]).find(Boolean) ?? null
        : repositoryClonedDirectory(repositorySourceAsset ?? asset) ?? repositoryLocalAsset?.value ?? null
      : null;
    return {
      asset,
      assetIds,
      label: workspaceAssetLabel(asset),
      repositoryCloned: asset.kind === 'repo' ? clonedDirectory !== null : null,
      repositoryCloneAssetId: repositorySourceAsset?.id ?? null,
      repositoryLocalPath: clonedDirectory
    };
  }).sort((left, right) => {
    if (left.asset.direction !== right.asset.direction) return left.asset.direction === 'in_scope' ? -1 : 1;
    return workspaceAssetKindOrder(left.asset.kind) - workspaceAssetKindOrder(right.asset.kind)
      || left.label.localeCompare(right.label);
  });
}

function workspaceAssetGroupKey(asset: ScopeAsset): string {
  if (asset.kind === 'repo') {
    const repositoryIdentity = repositoryIdentityFromAsset(asset);
    if (repositoryIdentity) return `${asset.direction}:repo:${repositoryIdentity}`;
  }
  return `${asset.direction}:${asset.kind}:${asset.id}`;
}

function preferredWorkspaceSurfaceAsset(assets: ScopeAsset[]): ScopeAsset {
  return assets.find((asset) => typeof asset.attributes?.displayName === 'string' && asset.attributes.displayName.trim())
    ?? assets.find((asset) => repositoryNameFromUrl(asset.value))
    ?? assets[0];
}

export function workspaceResearchSurfaceKinds(items: readonly WorkspaceResearchSurfaceItem[]): ScopeAssetKind[] {
  const representedKinds = new Set(items.map((item) => item.asset.kind));
  return WORKSPACE_ASSET_KINDS.filter((kind) => representedKinds.has(kind));
}

export function WorkspaceResourceDialog(props: Omit<Parameters<typeof WorkspaceResourceEditor>[0], 'presentation'>): JSX.Element {
  return <WorkspaceResourceEditor {...props} presentation="dialog" />;
}

export function WorkspaceResourceEditor({
  onEditStateChange,
  presentation = 'page',
  initialClonedDirectory,
  initialAsset,
  kind,
  onClose,
  onRemove,
  onSubmit
}: {
  onEditStateChange?: (state: { dirty: boolean; pending: boolean }) => void;
  presentation?: 'dialog' | 'page' | 'sidebar';
  initialClonedDirectory?: string | null;
  initialAsset: ScopeAsset | null;
  kind: ScopeAssetKind;
  onClose: () => void;
  onRemove?: () => Promise<void>;
  onSubmit: (asset: ScopeAssetInput) => Promise<void>;
}): JSX.Element {
  const editing = initialAsset !== null;
  const initialDisplayName = typeof initialAsset?.attributes?.displayName === 'string'
    ? initialAsset.attributes.displayName
    : '';
  const [value, setValue] = useState(initialAsset?.value ?? '');
  const [displayName, setDisplayName] = useState(initialDisplayName);
  const [clonedDirectory, setClonedDirectory] = useState(
    initialClonedDirectory ?? (initialAsset ? repositoryClonedDirectory(initialAsset) : null) ?? ''
  );
  const [direction, setDirection] = useState<ScopeAssetInput['direction']>(initialAsset?.direction ?? 'in_scope');
  const [sensitivity, setSensitivity] = useState(initialAsset?.sensitivity ?? 'internal');
  const [pendingAction, setPendingAction] = useState<'save' | 'remove' | null>(null);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [confirmLeave, setConfirmLeave] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const dirty = value !== (initialAsset?.value ?? '') || displayName !== initialDisplayName || clonedDirectory !== (initialClonedDirectory ?? (initialAsset ? repositoryClonedDirectory(initialAsset) : null) ?? '') || direction !== (initialAsset?.direction ?? 'in_scope') || sensitivity !== (initialAsset?.sensitivity ?? 'internal');
  useEffect(() => { onEditStateChange?.({ dirty, pending: pendingAction !== null }); }, [dirty, pendingAction, onEditStateChange]);
  useEffect(() => () => { onEditStateChange?.({ dirty: false, pending: false }); }, [onEditStateChange]);
  useEffect(() => {
    if (!dirty) return undefined;
    const preventUnload = (event: BeforeUnloadEvent): void => { event.preventDefault(); event.returnValue = ''; };
    window.addEventListener('beforeunload', preventUnload);
    return () => window.removeEventListener('beforeunload', preventUnload);
  }, [dirty]);

  const submit = async (): Promise<void> => {
    const trimmedValue = value.trim();
    if (!trimmedValue || pendingAction) return;
    setPendingAction('save');
    setSubmitError(null);
    try {
      const attributes: Record<string, unknown> = {
        ...(initialAsset?.attributes ?? {}),
        source: 'manual',
      };
      if (displayName.trim()) attributes.displayName = displayName.trim();
      else delete attributes.displayName;
      if (kind === 'repo') {
        const previousRepositoryUrl = typeof initialAsset?.attributes?.repositoryUrl === 'string'
          ? initialAsset.attributes.repositoryUrl.trim()
          : initialAsset?.value.trim() ?? '';
        attributes.repositoryUrl = trimmedValue;
        const referenceChanged = previousRepositoryUrl && previousRepositoryUrl.toLowerCase() !== trimmedValue.toLowerCase();
        if (referenceChanged) {
          clearRepositoryCheckoutAttributes(attributes);
        }
        const previousCheckout = initialClonedDirectory ?? (initialAsset ? repositoryClonedDirectory(initialAsset) : null) ?? '';
        if (clonedDirectory.trim() && (!referenceChanged || clonedDirectory.trim() !== previousCheckout.trim())) attributes.clonedDirectory = clonedDirectory.trim();
        else clearRepositoryCheckoutAttributes(attributes);
      }
      await onSubmit({ direction, kind, value: trimmedValue, sensitivity, attributes });
      if (presentation === 'dialog') onClose();
    } catch (caught) {
      setSubmitError(errorMessage(caught));
    } finally {
      setPendingAction(null);
    }
  };

  const remove = async (): Promise<void> => {
    if (!onRemove || pendingAction) return;
    setPendingAction('remove');
    setSubmitError(null);
    try {
      await onRemove();
      onClose();
    } catch (caught) {
      setSubmitError(errorMessage(caught));
    } finally {
      setPendingAction(null);
    }
  };

  const footer = (
    <>
          {onRemove ? (
            <button
              className="workspace-resource-remove-button modal-footer-leading"
              disabled={pendingAction !== null}
              onClick={() => setConfirmRemove(true)}
              type="button"
            >
              <Trash2 aria-hidden="true" size={15} />
              <span>{pendingAction === 'remove' ? 'Removing…' : 'Remove'}</span>
            </button>
          ) : null}
          <button className="primary-button" disabled={pendingAction !== null || !value.trim()} onClick={() => void submit()} type="button">
            {pendingAction === 'save' ? (editing ? 'Saving…' : 'Adding…') : (editing ? 'Save changes' : 'Add resource')}
          </button>
        </>
  );
  const pageLayout = presentation !== 'dialog';
  const form = (
    <form className={`modal-form workspace-resource-form${pageLayout ? ' is-page' : ''}`} onSubmit={(event) => { event.preventDefault(); void submit(); }}>
        <fieldset className={pageLayout ? 'settings-form-squircle' : undefined} disabled={pendingAction !== null}>
        <label className={pageLayout ? 'settings-form-control-row workspace-resource-control-row' : undefined}>
          <span>Resource type</span>
          <input disabled readOnly value={workspaceAssetKindLabel(kind)} />
        </label>
        <label className={pageLayout ? 'settings-form-control-row workspace-resource-control-row' : undefined}>
          <span>Reference</span>
          <input
            autoFocus
            onChange={(event) => setValue(event.target.value)}
            placeholder={workspaceAssetPlaceholder(kind)}
            required
            value={value}
          />
        </label>
        <label className={pageLayout ? 'settings-form-control-row workspace-resource-control-row' : undefined}>
          <span>Display name</span>
          <input onChange={(event) => setDisplayName(event.target.value)} placeholder="Optional" value={displayName} />
        </label>
        {kind === 'repo' ? (
          <label className={pageLayout ? 'settings-form-control-row workspace-resource-control-row' : undefined}>
            <span>Cloned directory</span>
            <input
              onChange={(event) => setClonedDirectory(event.target.value)}
              placeholder="Optional local checkout directory"
              value={clonedDirectory}
            />
          </label>
        ) : null}
        <div className={`workspace-resource-form-row${pageLayout ? ' is-page' : ''}`}>
          <label className={pageLayout ? 'settings-form-control-row workspace-resource-control-row' : undefined}>
            <span>Scope</span>
            <select onChange={(event) => setDirection(event.target.value as ScopeAssetInput['direction'])} value={direction}>
              <option value="in_scope">In scope</option>
              <option value="out_of_scope">Out of scope</option>
            </select>
          </label>
          <label className={pageLayout ? 'settings-form-control-row workspace-resource-control-row' : undefined}>
            <span>Sensitivity</span>
            <select onChange={(event) => setSensitivity(event.target.value)} value={sensitivity}>
              <option value="public">Public</option>
              <option value="internal">Internal</option>
              <option value="restricted">Restricted</option>
            </select>
          </label>
        </div>
        </fieldset>
        {submitError ? <p className="form-error" role="alert">{submitError}</p> : null}
      </form>
  );
  const removal = confirmRemove ? (
    <div className="resource-detail-confirm" role="alert">
      <p>Remove this resource from the workspace? Saved history and local files will remain.</p>
      <button className="secondary-button" disabled={pendingAction !== null} onClick={() => setConfirmRemove(false)} type="button">Keep resource</button>
      <button className="secondary-button" disabled={pendingAction !== null} onClick={() => void remove()} type="button">Confirm removal</button>
    </div>
  ) : null;
  if (presentation === 'dialog') return <Modal className="start-run-dialog workspace-resource-dialog" closeDisabled={pendingAction !== null} footer={footer} onClose={onClose} title={`${editing ? 'Edit' : 'Add'} ${workspaceAssetKindLabel(kind)}`}>{form}{removal}</Modal>;
  if (presentation === 'sidebar') return <div className="workspace-resource-editor is-sidebar">
    {form}
    {value.trim() !== initialAsset?.value.trim() ? <p>Changing the reference opens history for the new resource. Existing history remains associated with the previous reference.</p> : null}
    <div className="resource-detail-actions">{footer}</div>
    {removal}
  </div>;
  return (
    <div className="workspace-resource-editor">
      <header className="resource-detail-header">
        <button className="secondary-button" disabled={pendingAction !== null} onClick={() => dirty ? setConfirmLeave(true) : onClose()} type="button">Back to resources</button>
        <h2>{initialDisplayName || initialAsset?.value || workspaceAssetKindLabel(kind)}</h2>
      </header>
      {confirmLeave ? <div className="resource-detail-confirm" role="alert">
        <p>Discard unsaved resource changes?</p>
        <button className="secondary-button" onClick={() => setConfirmLeave(false)} type="button">Keep editing</button>
        <button className="secondary-button" onClick={onClose} type="button">Discard changes</button>
      </div> : null}
      {form}
      {value.trim() !== initialAsset?.value.trim() ? <p>Changing the reference opens history for the new resource. Existing history remains associated with the previous reference.</p> : null}
      <div className="resource-detail-actions">{footer}</div>
      {removal}
    </div>
  );
}

function workspaceAssetPlaceholder(kind: ScopeAssetKind): string {
  if (kind === 'repo') return 'https://github.com/owner/repository';
  if (kind === 'domain') return 'example.com';
  if (kind === 'service') return 'https://service.example.com';
  if (kind === 'documentation') return 'https://docs.example.com';
  return `Enter ${workspaceAssetKindLabel(kind).toLowerCase()} reference`;
}

function workspaceAssetLabel(asset: ScopeAsset): string {
  const displayName = typeof asset.attributes?.displayName === 'string' ? asset.attributes.displayName.trim() : '';
  if (displayName) return displayName;
  if (asset.kind === 'repo') {
    const repositoryName = repositoryNameFromAsset(asset);
    if (repositoryName) return repositoryName;
  }
  return asset.value;
}

function repositoryNameFromAsset(asset: ScopeAsset): string | null {
  const repositoryUrl = typeof asset.attributes?.repositoryUrl === 'string' ? asset.attributes.repositoryUrl : '';
  const urlName = repositoryNameFromUrl(repositoryUrl);
  if (urlName) return urlName;
  const directUrlName = repositoryNameFromUrl(asset.value);
  if (directUrlName) return directUrlName;
  return repositoryNameFromPath(asset.value);
}

function repositoryIdentityFromAsset(asset: ScopeAsset): string | null {
  const repositoryUrl = typeof asset.attributes?.repositoryUrl === 'string' ? asset.attributes.repositoryUrl : '';
  return repositoryIdentityFromUrl(repositoryUrl)
    ?? repositoryIdentityFromUrl(asset.value)
    ?? repositoryIdentityFromPath(asset.value);
}

function repositoryNameFromUrl(value: string): string | null {
  const trimmed = value.trim().replace(/\.git$/iu, '').replace(/\/+$/u, '');
  if (!trimmed || !/^[a-z][a-z0-9+.-]*:\/\//iu.test(trimmed)) return null;
  const name = trimmed.split('/').filter(Boolean).at(-1);
  return name && !/^[a-z][a-z0-9+.-]*:$/iu.test(name) ? name : null;
}

function repositoryIdentityFromUrl(value: string): string | null {
  const trimmed = value.trim().replace(/\.git$/iu, '').replace(/\/+$/u, '');
  if (!trimmed || !/^[a-z][a-z0-9+.-]*:\/\//iu.test(trimmed)) return null;
  try {
    const parsed = new URL(trimmed);
    const pathname = parsed.pathname.split('/').filter(Boolean).join('/').toLowerCase();
    return pathname ? `${parsed.hostname.toLowerCase()}/${pathname}` : null;
  } catch {
    return null;
  }
}

function repositoryNameFromPath(value: string): string | null {
  const parts = value.replace(/[\\/]+$/u, '').split(/[\\/]/u).filter(Boolean);
  const leaf = parts.at(-1)?.replace(/\.git$/iu, '');
  const parent = parts.at(-2);
  const materializedName = parent ? repositoryNameFromMaterializedSlug(parent) : null;
  if (materializedName && (!leaf || leaf === 'default' || /^[A-Za-z0-9_.-]+-[a-f0-9]{12}$/u.test(leaf))) {
    return materializedName;
  }
  return leaf || materializedName;
}

function repositoryIdentityFromPath(value: string): string | null {
  const parts = value.replace(/[\\/]+$/u, '').split(/[\\/]/u).filter(Boolean);
  const leaf = parts.at(-1)?.replace(/\.git$/iu, '') ?? '';
  const parent = parts.at(-2) ?? '';
  const materializedIdentity = repositoryIdentityFromMaterializedSlug(parent);
  if (materializedIdentity && (!leaf || leaf === 'default' || /^[A-Za-z0-9_.-]+-[a-f0-9]{12}$/u.test(leaf))) {
    return materializedIdentity;
  }
  return repositoryIdentityFromMaterializedSlug(leaf);
}

function repositoryNameFromMaterializedSlug(value: string): string | null {
  const segments = value.split('_').filter(Boolean);
  if (segments.length < 3 || !/^(?:github|gitlab)\.com$/iu.test(segments[0])) return null;
  const name = segments.at(-1)?.replace(/\.git$/iu, '');
  return name || null;
}

function repositoryIdentityFromMaterializedSlug(value: string): string | null {
  const segments = value.split('_').filter(Boolean);
  if (segments.length < 3 || !/^(?:github|gitlab)\.com$/iu.test(segments[0])) return null;
  return `${segments[0].toLowerCase()}/${segments.slice(1).join('/').replace(/\.git$/iu, '').toLowerCase()}`;
}

function workspaceAssetKindOrder(kind: ScopeAssetKind): number {
  const order: ScopeAssetKind[] = ['repo', 'documentation', 'binary', 'service', 'domain', 'other'];
  return order.indexOf(kind);
}

function clearRepositoryCheckoutAttributes(attributes: Record<string, unknown>): void {
  for (const key of [
    'clonedDirectory',
    'cloneSource',
    'sourceStorage',
    'sourceReferenceVersion',
    'head',
    'materializedRef',
    'cloned',
    'headRefName',
    'headDescribe',
    'requestedRefHead',
    'requestedRefMatchesHead'
  ]) {
    delete attributes[key];
  }
}
