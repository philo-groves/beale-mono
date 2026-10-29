import { useEffect, useMemo, useState } from 'react';
import type { JSX } from 'react';
import type { BealeAppServerWorkspaceDetails, BealeAppServerWorkspaceDocument, BealeWorkspaceResearchClaim, BealeWorkspaceResearchTrack } from '@beale/app-server-runtime/protocol';
import type {
  AppServerFindingSummary, AppServerMemorySummary, AppServerMemoryNodeSummary,
  AppServerReportSummary, AppServerRunbookSummary, ResearchProviderModelCatalog, ScopeAssetKind, WorkspaceScopeVersion
} from '../../../shared/types';
import { isResearchKitId } from '../../../shared/researchKits';
import { ResearchSidePanel } from '../research/MemorySidePanel';
import { WorkspaceUnderstandingView } from '../workspaces/WorkspaceUnderstandingView';
import { WorkspaceSurfaceDetails } from '../workspaces/WorkspaceSurfaceDetails';
import type { WorkspaceDashboardView } from '../workspaces/WorkspaceUnderstandingView';

const RESOURCE_KINDS: readonly ScopeAssetKind[] = ['repo', 'documentation', 'binary', 'service', 'domain', 'other'];

function remoteScope(details: BealeAppServerWorkspaceDetails): WorkspaceScopeVersion | null {
  const scope = details.scope;
  if (!scope) return null;
  return {
    id: scope.id, version: 1, status: 'active', workspaceName: scope.name,
    scopeOwner: scope.owner, descriptionMarkdown: scope.descriptionMarkdown,
    rulesMarkdown: '', activeFrom: '', expiresAt: scope.expiresAt,
    createdAt: '', createdBy: '',
    assets: scope.resources.map((resource) => ({
      id: resource.id, scopeVersionId: scope.id, createdAt: '',
      kind: RESOURCE_KINDS.includes(resource.kind as ScopeAssetKind) ? resource.kind as ScopeAssetKind : 'other',
      direction: resource.direction, value: resource.value ?? 'Private resource',
      sensitivity: resource.sensitivity
    }))
  };
}

function remoteClaim(claim: BealeWorkspaceResearchClaim, workspaceId: string): AppServerFindingSummary {
  const maturity = claim.maturity === 'reported' ? 'verified'
    : claim.maturity === 'proposed' ? 'lead'
      : ['lead', 'observed', 'reproduced', 'verified', 'refuted'].includes(claim.maturity)
        ? claim.maturity as AppServerFindingSummary['maturity'] : 'lead';
  const status = ['hypothesis', 'observed', 'reproduced', 'verified', 'report_ready', 'disclosed', 'stale', 'rejected'].includes(claim.status)
    ? claim.status as AppServerFindingSummary['status'] : 'hypothesis';
  return {
    id: claim.id, surfaceId: claim.surfaceId ?? null, workspaceId, subjectId: '', memoryNodeId: null, originSessionId: null,
    projection: claim.projection, maturity,
    rating: claim.rating, classification: claim.classification,
    title: claim.title, summary: claim.summary, impact: claim.impact,
    stepsToReproduce: claim.stepsToReproduce,
    securityTracking: claim.securityTracking ? {
      ...claim.securityTracking,
      surfaceIds: claim.securityTracking.surfaceIds ?? [],
      progressBlocker: claim.securityTracking.progressBlocker ?? null,
      reachability: { ...claim.securityTracking.reachability, evidenceIds: [], assessorId: null,
        sourceRevision: null, environmentFingerprint: null },
      cvssAssessments: claim.securityTracking.cvssAssessments.map((assessment) => ({
        ...assessment, assessorId: '', environmentFingerprint: null
      }))
    } : null, status,
    operatorOverride: false,
    staleFromStatus: claim.staleFromStatus && ['hypothesis', 'observed', 'reproduced', 'verified', 'report_ready', 'disclosed', 'stale', 'rejected'].includes(claim.staleFromStatus)
      ? claim.staleFromStatus as AppServerFindingSummary['staleFromStatus'] : null,
    confidence: claim.confidence, sourceRevision: null,
    environmentFingerprint: null, reproductionRunbookId: null, reportId: null,
    disclosureReference: null, staleReason: null, duplicateOfClaimId: null,
    duplicateMarkedAt: null, duplicateClaims: [], evidence: [], transitions: [], authors: [],
    createdAt: claim.createdAt, updatedAt: claim.updatedAt, revision: claim.revision
  };
}

function remoteMemory(details: BealeAppServerWorkspaceDetails): AppServerMemorySummary {
  const { workspaceId } = details.workspace;
  const name = details.scope?.name ?? details.workspace.name;
  const nodes: AppServerMemoryNodeSummary[] = details.memory.nodes.map((node) => ({
    id: node.id, surfaceId: node.surfaceId ?? null, sessionIds: node.sessionIds, workspaces: [{ id: workspaceId, name }],
    subjectId: '', subjectName: details.researchSubject, type: node.type, title: node.title,
    summary: node.summary, body: '', status: node.status, confidence: node.confidence,
    assetIds: [], tags: node.tags, attributes: {}, evidenceRefs: [],
    createdAt: node.createdAt, updatedAt: node.updatedAt, revision: node.revision,
    duplicateMemories: node.duplicateMemories, authors: []
  }));
  const runbooks: AppServerRunbookSummary[] = details.runbooks.map((runbook) => ({
    id: runbook.id, workspaceId, workspaceName: name, subjectId: null, subjectName: details.researchSubject || null,
    sessionId: null, title: runbook.title, purpose: runbook.purpose, artifactId: '',
    revision: 1, contentRevision: 1, execution: { runCount: 0, completedRunCount: 0,
      executedCellCount: 0, latest: null, latestSuccessfulRunId: null },
    revisions: [], createdAt: runbook.updatedAt, updatedAt: runbook.updatedAt, authors: []
  }));
  const reports: AppServerReportSummary[] = details.reports.map((report) => ({
    id: report.id, workspaceId, workspaceName: name, subjectId: null, subjectName: details.researchSubject || null,
    sessionId: null, title: report.title, summary: report.summary, status: report.status,
    triageStatus: 'editing', artifactId: '', submissionPacket: null, recording: null,
    revision: 1, revisions: [], createdAt: report.updatedAt, updatedAt: report.updatedAt, authors: []
  }));
  const leads = details.memory.leads.map((claim) => remoteClaim(claim, workspaceId));
  const findings = details.memory.findings.map((claim) => remoteClaim(claim, workspaceId));
  return {
    status: details.memory.status === 'ready' || details.memory.status === 'empty'
      || details.memory.status === 'missing' || details.memory.status === 'error' ? details.memory.status : 'ready',
    source: 'app_server_sqlite', contextWorkspaceId: workspaceId, contextSubjectId: '',
    databasePath: '', storageRoot: '', artifactDirectoryPath: '', databaseSizeBytes: 0,
    nodeCount: details.memory.nodeCount, edgeCount: 0, evidenceRefCount: 0,
    storageArtifactCount: 0, runbookCount: runbooks.length, reportCount: reports.length,
    latestNodeUpdatedAt: nodes.reduce<string | null>((latest, node) =>
      !latest || node.updatedAt > latest ? node.updatedAt : latest, null),
    nodeTypeCounts: details.memory.nodeTypeCounts, nodeStatusCounts: {},
    nodes, edges: [], runbooks, reports, leads, findings,
    campaign: { nodes: [], edges: [], coverageGaps: [], contradictions: [],
      momentum: { state: 'empty', reason: '', supportingNodeIds: [] }, nextActions: [],
      counts: { leads: leads.length, findings: findings.length,
        verifiedFindings: findings.filter((claim) => claim.maturity === 'verified').length,
        disclosedFindings: 0, coverageGaps: 0, contradictions: 0 } },
    directories: [], lastError: null
  };
}

export function RemoteWorkspaceDetails({ serverId, details, providerModelCatalog = [], initialView = 'board', onViewChange, onOpenSession, onStartTrack, onTrackResumed }: {
  serverId: string; details: BealeAppServerWorkspaceDetails; initialView?: WorkspaceDashboardView;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  onViewChange?: (viewName: string) => void;
  onOpenSession?: (sessionId: string) => void;
  onStartTrack?: () => void;
  onTrackResumed?: (track: BealeWorkspaceResearchTrack) => void;
}): JSX.Element {
  const [selected, setSelected] = useState<{ kind: 'claim' | 'memory' | 'runbook' | 'report' | 'surface'; id: string } | null>(null);
  const [documentState, setDocumentState] = useState<{ key: string; value: BealeAppServerWorkspaceDocument | null; error: string | null } | null>(null);
  const scope = useMemo(() => remoteScope(details), [details]);
  const memory = useMemo(() => remoteMemory(details), [details]);
  const workspaceName = scope?.workspaceName ?? details.workspace.name;
  const selectedRunbook = selected?.kind === 'runbook' ? memory.runbooks.find((item) => item.id === selected.id) ?? null : null;
  const selectedReport = selected?.kind === 'report' ? memory.reports.find((item) => item.id === selected.id) ?? null : null;
  const selectedSurface = selected?.kind === 'surface' ? details.surfaceMap.surfaces.find((item) => item.id === selected.id) ?? null : null;
  const documentKey = selectedRunbook ? `runbook:${selectedRunbook.id}` : selectedReport ? `report:${selectedReport.id}` : null;
  const open = (kind: 'claim' | 'memory' | 'runbook' | 'report' | 'surface', id: string): void => {
    setDocumentState(null);
    setSelected({ kind, id });
  };
  const close = (): void => { setSelected(null); setDocumentState(null); };
  useEffect(() => {
    if (selected?.kind === 'surface' && !selectedSurface) close();
  }, [selected, selectedSurface]);
  useEffect(() => {
    if (!documentKey || !selected || (selected.kind !== 'runbook' && selected.kind !== 'report')) return undefined;
    let active = true;
    setDocumentState({ key: documentKey, value: null, error: null });
    void window.beale.getFleetRemoteWorkspaceDocument(serverId, details.workspace.workspaceId, selected.kind, selected.id)
      .then((value) => { if (active) setDocumentState({ key: documentKey, value, error: null }); })
      .catch((caught: unknown) => { if (active) setDocumentState({ key: documentKey, value: null, error: caught instanceof Error ? caught.message : String(caught) }); });
    return () => { active = false; };
  }, [serverId, details.workspace.workspaceId, documentKey]);
  const visibleDocument = documentState?.key === documentKey ? documentState : null;
  const documentLoading = Boolean(documentKey && (!visibleDocument || !visibleDocument.value && !visibleDocument.error));
  return <div className={`main-session-grid workspace-context${selected ? ' research-details-open' : ' workspace-main-only'}${selected?.kind === 'claim' ? ' finding-details-open' : ''}`} data-session-view-state="workspace">
    <WorkspaceUnderstandingView
      busy={false}
      initialView={initialView}
      rightAreaExpanded={selected !== null}
      readOnly
      remoteTargets={details.targets}
      trackServerId={serverId}
      surfaceMap={details.surfaceMap}
      appServerMemory={memory}
      activeScope={scope}
      workspaceRules={details.rules.map((rule) => ({
        ...rule, workspaceId: details.workspace.workspaceId, createdAt: '', createdBy: ''
      }))}
      researchKitId={isResearchKitId(details.workspace.researchKitId) ? details.workspace.researchKitId : 'general'}
      researchProfileLabel={details.workspace.researchProfileId === 'security-research'
        ? 'Security' : details.workspace.researchProfileId}
      researchSubjectName={details.researchSubject}
      workspaceId={details.workspace.workspaceId}
      workspaceName={workspaceName}
      runs={[]}
      onOpenSession={onOpenSession}
      onStartTrack={onStartTrack}
      onTrackResumed={onTrackResumed}
      onActiveViewChange={onViewChange}
      onViewSelectionChange={(view) => { if (view !== 'surfaces' && selected?.kind === 'surface') close(); }}
      selectedSurfaceId={selectedSurface?.id ?? null}
      onOpenSurface={(surface) => open('surface', surface.id)}
      onOpenClaim={(id) => open('claim', id)}
      openClaimId={selected?.kind === 'claim' ? selected.id : null}
      onOpenMemory={(id) => open('memory', id)}
      onOpenRunbook={(id) => open('runbook', id)}
      onOpenReport={(report) => open('report', report.id)}
    />
    <div className="research-side-resize-handle" aria-hidden="true" />
    <div className="research-side-column">
      {selectedSurface ? <WorkspaceSurfaceDetails surface={selectedSurface} map={details.surfaceMap}
        workspaceId={details.workspace.workspaceId} activeScope={scope} providerModelCatalog={providerModelCatalog}
        leads={memory.leads} findings={memory.findings} onOpenClaim={(id) => open('claim', id)} readOnly onClose={close} /> : <ResearchSidePanel
        browserContextsEnabled={false}
        detail={null}
        events={[]}
        memory={memory}
        providerModelCatalog={[]}
        runId={`workspace:${details.workspace.workspaceId}`}
        runStatus={null}
        expanded={Boolean(selected)}
        selectedRunbook={selectedRunbook}
        selectedRunbookDocument={visibleDocument?.value?.kind === 'runbook' ? visibleDocument.value.document : null}
        selectedRunbookId={selectedRunbook?.id ?? null}
        selectedClaimId={selected?.kind === 'claim' ? selected.id : null}
        selectedMemoryNodeId={selected?.kind === 'memory' ? selected.id : null}
        runbookLoading={documentLoading}
        runbookError={visibleDocument?.error ?? null}
        selectedReport={selectedReport}
        selectedReportDocument={visibleDocument?.value?.kind === 'report' ? visibleDocument.value.document : null}
        selectedReportId={selectedReport?.id ?? null}
        reportLoading={documentLoading}
        reportError={visibleDocument?.error ?? null}
        selectedSubagentPath={null}
        searchHighlightQuery=""
        onSelectSubagent={() => undefined}
        onOpenClaim={(id) => open('claim', id)}
        onOpenRunbook={(id) => open('runbook', id)}
        onOpenReport={(id) => open('report', id)}
        onBackToRunbooks={close}
        onBackToReports={close}
        onBackToClaim={close}
        onBackToMemory={close}
        onBackToSubagents={() => undefined}
        onExpandedChange={(expanded) => { if (!expanded) close(); }}
        viewSpace="workspace"
      />}
    </div>
  </div>;
}
