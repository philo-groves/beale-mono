import { useId, useState, type JSX } from 'react';
import { ArrowLeft, ArrowRight, Download } from 'lucide-react';
import type { BealeWorkspaceSurfaceMap } from '@beale/app-server-runtime/protocol';
import type { AppServerFindingSummary, AppServerMemoryNodeSummary, RepositoryCloneMode, ResearchProviderModelCatalog, ScopeAssetInput, WorkspaceScopeVersion } from '@shared/types';
import type { MapSurface } from '../../view-models/surfaceMap';
import { errorMessage } from '../../lib/errors';
import { researchModelDisplayName, traceLabel } from '../../lib/formatting';
import { campaignClaimRatingPresentation } from '../../view-models/campaignClaims';
import { Modal } from '../../app/Modal';
import { ProviderIcon } from '../../app/ProviderIcon';
import { ResourcePriorArtView } from './ResourcePriorArtView';
import { SurfaceThreatDiagram } from './SurfaceThreatDiagram';
import { WorkspaceResourceEditor, workspaceResearchSurfaceItems } from './WorkspaceUnderstandingView';

const DETAIL_VIEWS = ['Breakdown', 'Recon', 'Threat Model', 'Analysis', 'Resources'] as const;
type DetailView = typeof DETAIL_VIEWS[number];

const RANKING_FACTORS = [
  { key: 'impact', label: 'Impact', x: 180, y: 16 },
  { key: 'reachability', label: 'Reachability', x: 304, y: 105 },
  { key: 'relevance', label: 'Relevance', x: 262, y: 266 },
  { key: 'exposure', label: 'Exposure', x: 98, y: 266 },
  { key: 'precedence', label: 'Precedence', x: 56, y: 105 },
] as const;
type SurfaceRanking = NonNullable<NonNullable<MapSurface['foundation']>['ranking']>;

function rankingPoint(index: number, radius: number): [number, number] {
  const angle = (-90 + index * 72) * Math.PI / 180;
  return [Number((180 + Math.cos(angle) * radius).toFixed(2)),
    Number((153 + Math.sin(angle) * radius).toFixed(2))];
}

function SurfaceRankingChart({ ranking }: { ranking: SurfaceRanking | null | undefined }): JSX.Element {
  if (!ranking) return <section className="workspace-surface-details-section">
    <h3>Surface Ranking</h3><p className="workspace-surface-details-muted">No priority scores recorded.</p>
  </section>;
  const average = RANKING_FACTORS.reduce((sum, factor) => sum + ranking[factor.key].score, 0) / RANKING_FACTORS.length;
  const description = RANKING_FACTORS.map((factor) => `${factor.label} ${ranking[factor.key].score} of 10`).join(', ');
  return <section className="workspace-surface-details-section workspace-surface-ranking">
    <h3>Surface Ranking</h3>
    <figure>
      <svg viewBox="0 0 360 300" role="img" aria-label={`Surface ranking: ${description}`}>
        {[2, 4, 6, 8, 10].map((level) => <polygon key={level} className="workspace-surface-ranking-grid"
          points={RANKING_FACTORS.map((_, index) => rankingPoint(index, 10.4 * level).join(',')).join(' ')} />)}
        {RANKING_FACTORS.map((_, index) => {
          const [x, y] = rankingPoint(index, 104);
          return <line key={index} className="workspace-surface-ranking-axis" x1="180" y1="153" x2={x} y2={y} />;
        })}
        <polygon className="workspace-surface-ranking-area"
          points={RANKING_FACTORS.map((factor, index) => rankingPoint(index, ranking[factor.key].score * 10.4).join(',')).join(' ')} />
        {RANKING_FACTORS.map((factor, index) => {
          const [x, y] = rankingPoint(index, ranking[factor.key].score * 10.4);
          return <g key={factor.key}>
          <title>{factor.label}: {ranking[factor.key].score}/10. {ranking[factor.key].basis}</title>
          <circle className="workspace-surface-ranking-point" cx={x} cy={y} r="3" />
          <text className="workspace-surface-ranking-label" x={factor.x} y={factor.y} textAnchor="middle">{factor.label}</text>
          <text className="workspace-surface-ranking-score" x={factor.x} y={factor.y + 17} textAnchor="middle">{ranking[factor.key].score}/10</text>
        </g>})}
      </svg>
      <figcaption>Average {average.toFixed(1)}/10 · Higher means more priority</figcaption>
    </figure>
    <details className="workspace-surface-ranking-basis"><summary>Assessment basis</summary>
      <dl>{RANKING_FACTORS.map((factor) => <div key={factor.key}>
        <dt>{factor.label} · {ranking[factor.key].score}/10</dt><dd>{ranking[factor.key].basis}</dd>
      </div>)}</dl>
    </details>
  </section>;
}

function DetailText({ label, value }: { label: string; value: string | undefined }): JSX.Element | null {
  return value?.trim() ? <div className="workspace-surface-detail-text"><h4>{label}</h4><p>{value}</p></div> : null;
}

function ThreatTextList({ title, value }: { title: string; value: readonly string[] | string | undefined }): JSX.Element {
  const items = typeof value === 'string' ? value.trim() ? [value.trim()] : []
    : (value ?? []).map((item) => item.trim()).filter(Boolean);
  return <section className="workspace-surface-details-section">
    <h3>{title}</h3>
    {items.length ? <ul className="workspace-surface-threat-list">{items.map((item, index) => <li key={`${index}:${item}`}>{item}</li>)}</ul>
      : <p className="workspace-surface-details-muted">No {title.toLocaleLowerCase()} recorded.</p>}
  </section>;
}

function ThreatMemoryList({ title, nodes, onOpenMemory }: { title: string; nodes: readonly AppServerMemoryNodeSummary[];
  onOpenMemory?: (nodeId: string) => void }): JSX.Element {
  return <section className="workspace-surface-details-section">
    <h3>{title}</h3>
    {nodes.length ? <ul className="workspace-surface-threat-list">{nodes.map((node) => <li key={node.id} className="workspace-surface-threat-memory-item">
      <span className="workspace-surface-threat-memory-content">
        <span>{node.title}</span>
        <small>{traceLabel(node.status)}</small>
      </span>
      {onOpenMemory ? <button className="workspace-surface-threat-open" type="button"
        aria-label={`Open ${node.type} ${node.title}`} title={`Open ${node.title}`} onClick={() => onOpenMemory(node.id)}>
        <ArrowRight aria-hidden="true" size={16} />
      </button> : null}
    </li>)}</ul> : <p className="workspace-surface-details-muted">No {title.toLocaleLowerCase()} recorded.</p>}
  </section>;
}

export function WorkspaceSurfaceDetails({ surface, map, workspaceId = '', activeScope = null, repositoryCheckouts,
  providerModelCatalog = [], leads = [], findings = [], memories = [],
  readOnly = false, initialView = 'Breakdown', onChangeResource, onCloneRepository, onOpenClaim, onOpenMemory, onClose }: {
  surface: MapSurface;
  map: BealeWorkspaceSurfaceMap;
  workspaceId?: string;
  activeScope?: WorkspaceScopeVersion | null;
  repositoryCheckouts?: Readonly<Record<string, string>>;
  providerModelCatalog?: readonly ResearchProviderModelCatalog[];
  leads?: readonly AppServerFindingSummary[];
  findings?: readonly AppServerFindingSummary[];
  memories?: readonly AppServerMemoryNodeSummary[];
  readOnly?: boolean;
  initialView?: DetailView;
  onChangeResource?: (assetIds: string[], asset: ScopeAssetInput | null) => Promise<void>;
  onCloneRepository?: (assetId: string, mode: RepositoryCloneMode) => Promise<void>;
  onOpenClaim?: (claimId: string) => void;
  onOpenMemory?: (nodeId: string) => void;
  onClose: () => void;
}): JSX.Element {
  const agents = map.agents.filter((agent) => agent.surfaceId === surface.id);
  const surfaceLeads = leads.filter((lead) => lead.workspaceId === map.workspaceId && lead.projection === 'lead'
    && lead.status !== 'rejected'
    && !lead.duplicateOfClaimId && (lead.surfaceId === surface.id
      || lead.securityTracking?.surfaceIds.includes(surface.id)));
  const childrenByParent = new Map<string, string[]>();
  const surfaceNames = new Map(map.surfaces.map((item) => [item.id, item.name]));
  const surfaceById = new Map(map.surfaces.map((item) => [item.id, item]));
  let excludedBy = surface.foundation?.scopeAssessment?.decision === 'not_applicable' ? surface : null;
  let scopeAncestor = surface.parentId ? surfaceById.get(surface.parentId) : null;
  const visitedScopeAncestors = new Set<string>();
  while (scopeAncestor && !visitedScopeAncestors.has(scopeAncestor.id)) {
    visitedScopeAncestors.add(scopeAncestor.id);
    if (scopeAncestor.foundation?.scopeAssessment?.decision === 'not_applicable') excludedBy = scopeAncestor;
    scopeAncestor = scopeAncestor.parentId ? surfaceById.get(scopeAncestor.parentId) : null;
  }
  const scopeAssessment = excludedBy?.foundation?.scopeAssessment ?? surface.foundation?.scopeAssessment;
  for (const item of map.surfaces) if (item.parentId) {
    childrenByParent.set(item.parentId, [...(childrenByParent.get(item.parentId) ?? []), item.id]);
  }
  const descendantIds = new Set<string>();
  const pending = [surface.id];
  while (pending.length) {
    const id = pending.pop()!;
    if (descendantIds.has(id)) continue;
    descendantIds.add(id);
    pending.push(...(childrenByParent.get(id) ?? []));
  }
  const surfaceFindings = findings.flatMap((finding) => {
    if (finding.workspaceId !== map.workspaceId || finding.projection !== 'finding' || finding.duplicateOfClaimId) return [];
    const assignedSurfaceId = finding.surfaceId && descendantIds.has(finding.surfaceId)
      ? finding.surfaceId : finding.securityTracking?.surfaceIds.find((id) => descendantIds.has(id));
    return assignedSurfaceId && descendantIds.has(assignedSurfaceId)
      ? [{ finding, assignedSurfaceId }] : [];
  });
  const workspaceMemories = memories.filter((node) => node.workspaces.some((workspace) => workspace.id === map.workspaceId)
    && node.status !== 'rejected' && !node.duplicateOfMemoryId);
  const surfaceMemories = workspaceMemories.filter((node) => node.surfaceId === surface.id);
  const surfaceTrajectories = workspaceMemories.filter((node) => node.type === 'trajectory'
    && node.surfaceId && descendantIds.has(node.surfaceId));
  const status = surface.resourceDirection === 'out_of_scope' ? 'Out of scope'
    : excludedBy ? 'Not applicable'
    : surface.progressBlocker ? 'Blocked' : surface.coverageState.covered ? 'Covered'
    : surface.firstTouchedAt ? 'In progress' : 'Untouched';
  const firstTouchedModel = surface.firstTouchedModel?.trim() || null;
  const firstTouchedModelName = firstTouchedModel
    ? researchModelDisplayName(surface.firstTouchedProvider ?? null, firstTouchedModel, providerModelCatalog) : 'Unknown';
  const resourceItem = surface.scopeAssetId ? workspaceResearchSurfaceItems(activeScope?.assets ?? [], repositoryCheckouts)
    .find((item) => item.assetIds.includes(surface.scopeAssetId!)) ?? null : null;
  const detailViews: readonly DetailView[] = surface.parentId === null && surface.resourceKind
    ? DETAIL_VIEWS : DETAIL_VIEWS.filter((view) => view !== 'Resources');
  const tabId = useId();
  const [selection, setSelection] = useState<{ surfaceId: string; view: DetailView }>({
    surfaceId: surface.id, view: detailViews.includes(initialView) ? initialView : 'Breakdown'
  });
  const activeView = selection.surfaceId === surface.id ? selection.view : 'Breakdown';
  const [resourceEditState, setResourceEditState] = useState({ dirty: false, pending: false });
  const [pendingNavigation, setPendingNavigation] = useState<DetailView | 'back' | null>(null);
  const [cloneMode, setCloneMode] = useState<RepositoryCloneMode | null>(null);
  const [clonePending, setClonePending] = useState(false);
  const [cloneError, setCloneError] = useState<string | null>(null);
  const navigate = (view: DetailView | 'back'): void => {
    if (resourceEditState.pending) return;
    if (resourceEditState.dirty && activeView === 'Resources') { setPendingNavigation(view); return; }
    if (view === 'back') onClose();
    else setSelection({ surfaceId: surface.id, view });
  };

  return <aside className="workspace-surface-details main-session-side" aria-label="Surface details">
    <header className="workspace-surface-details-header research-side-nested-header is-campaign-item">
      <button className="is-icon-only" type="button" onClick={() => navigate('back')} aria-label="Back to Surfaces" title="Back to Surfaces">
        <ArrowLeft aria-hidden="true" size={17} />
      </button>
      <span className="research-side-nested-identity">
        <span className="research-side-nested-primary">
          <span className="research-side-nested-name" title={surface.name}>{surface.name}</span>
          <span className={`workspace-surface-details-status${surface.resourceDirection === 'out_of_scope' ? '' : surface.progressBlocker ? ' blocked' : surface.coverageState.covered ? ' covered' : ''}`}
            aria-label={`Surface status: ${status}`}>{status}</span>
        </span>
      </span>
    </header>
    <div className="workspace-surface-details-meta">
      <span className="workspace-surface-first-touch" title={surface.firstTouchedAt ? `First touched ${new Date(surface.firstTouchedAt).toLocaleString()}` : undefined}>
        {surface.firstTouchedAt ? <><span>First touched by</span><span className="memory-catalog-item-author">
          <span className="memory-catalog-item-author-provider">
            <ProviderIcon provider={surface.firstTouchedProvider || firstTouchedModel} size={15} aria-hidden="true" />
          </span>
          <span className="memory-catalog-item-author-model">{firstTouchedModelName}</span>
        </span></> : 'No first touch recorded'}
      </span>
      <span>{surface.updatedAt
        ? <>Updated <time dateTime={surface.updatedAt}>{new Date(surface.updatedAt).toLocaleString()}</time></>
        : 'Last update unknown'}</span>
    </div>
    <div className="workspace-surface-details-tabs" role="tablist" aria-label="Surface detail views">
      {detailViews.map((view, index) => <button key={view} id={`${tabId}-${view}`} type="button" role="tab"
        aria-selected={activeView === view} aria-controls={`${tabId}-panel`} tabIndex={activeView === view ? 0 : -1}
        onClick={() => navigate(view)}
        onKeyDown={(event) => {
          const next = event.key === 'ArrowRight' ? (index + 1) % detailViews.length
            : event.key === 'ArrowLeft' ? (index - 1 + detailViews.length) % detailViews.length
              : event.key === 'Home' ? 0 : event.key === 'End' ? detailViews.length - 1 : null;
          if (next === null) return;
          event.preventDefault();
          navigate(detailViews[next]!);
          event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
        }}>{view}</button>)}
    </div>
    <div className="workspace-surface-details-scroll" id={`${tabId}-panel`} role="tabpanel" aria-labelledby={`${tabId}-${activeView}`}>
      {activeView === 'Breakdown' ? <>
      <section className="workspace-surface-details-section">
        <h3>About</h3>
        <p className="workspace-surface-details-description">{surface.description.trim() || 'No description recorded.'}</p>
      </section>

      {scopeAssessment ? <section className="workspace-surface-details-section">
        <h3>Scope applicability</h3>
        <p>{scopeAssessment.decision === 'applicable' ? 'Applicable' : 'Not applicable'}
          {excludedBy && excludedBy.id !== surface.id ? ` via ${excludedBy.name}` : ''}</p>
        <p className="workspace-surface-details-description">{scopeAssessment.basis}</p>
        <ul>{scopeAssessment.references.map((reference) => <li key={reference}>{reference}</li>)}</ul>
      </section> : null}

      <SurfaceRankingChart ranking={surface.foundation?.ranking} />

      {surface.progressBlocker ? <section className="workspace-surface-details-section workspace-surface-details-blocker">
        <h3>Progress Blocker</h3><p>{surface.progressBlocker.reason}</p>
        <small>Recorded by {surface.progressBlocker.actorId} on {new Date(surface.progressBlocker.setAt).toLocaleString()}</small>
      </section> : null}

      {agents.length ? <section className="workspace-surface-details-section">
        <h3>Active Agents</h3>
        <ul className="workspace-surface-details-agents">{agents.map((agent) => <li key={`${agent.sessionId}:${agent.agentId}`}>
          <strong>{agent.agentId === 'root' ? 'Lead' : agent.agentId}</strong><span>{agent.sessionTitle}</span>
        </li>)}</ul>
      </section> : null}
      </> : activeView === 'Recon' ? <>
        <section className="workspace-surface-details-section">
          <h3>Discovery</h3>
          {surface.foundation?.discovery ? <p className="workspace-surface-details-description">{surface.foundation.discovery}</p>
            : <p className="workspace-surface-details-muted">No discovery note recorded.</p>}
        </section>
        <section className="workspace-surface-details-section workspace-surface-recon-list">
          <h3>High Value Signals</h3>
          {surface.foundation?.highValueSignals?.length ? <ul>{surface.foundation.highValueSignals.map((signal, index) => <li key={`${signal.kind}:${index}`}>
            <strong>{signal.kind.replaceAll('_', ' ')}</strong><p>{signal.basis}</p>
          </li>)}</ul> : <p className="workspace-surface-details-muted">None recorded.</p>}
        </section>
        <section className="workspace-surface-details-section workspace-surface-recon-list">
          <h3>Tech Stack</h3>
          {surface.foundation?.techStack?.length ? <ul>{surface.foundation.techStack.map((item, index) => <li key={`${item.technology}:${index}`}>
            <strong>{item.technology}</strong>
            {item.role ? <p>{item.role}</p> : null}
            <dl>
              {item.version ? <><dt>Version</dt><dd>{item.version}</dd></> : null}
              {item.environment ? <><dt>Environment</dt><dd>{item.environment}</dd></> : null}
              {item.evidence ? <><dt>Evidence</dt><dd>{item.evidence}</dd></> : null}
              {item.confidence !== 'unknown' ? <><dt>Basis</dt><dd>{item.confidence}</dd></> : null}
            </dl>
          </li>)}</ul> : <p className="workspace-surface-details-muted">No technologies mapped.</p>}
        </section>
        <section className="workspace-surface-details-section workspace-surface-recon-list">
          <h3>Assets</h3>
          {surface.foundation?.assets?.length ? <ul className="workspace-surface-assets">{surface.foundation.assets.map((asset, index) => <li key={`${asset.name}:${index}`}>
            <div className="workspace-surface-asset-row">
              <span>{asset.name}</span>
              <span className="workspace-surface-asset-status" data-status={asset.status}>{asset.status === 'in_progress' ? 'In Progress' : asset.status === 'complete' ? 'Complete' : 'Untouched'}</span>
            </div>
          </li>)}</ul> : <p className="workspace-surface-details-muted">No research assets recorded.</p>}
        </section>
        <section className="workspace-surface-details-section workspace-surface-recon-list">
          <h3>Bug History</h3>
          {surface.foundation?.bugHistory?.entries?.length ? <ul>{surface.foundation.bugHistory.entries.map((entry, index) => <li key={`${entry.identifier}:${index}`}>
            <strong>{entry.identifier}</strong>
            <p>{entry.impact}</p>
            <dl>
              {entry.affectedVersions ? <><dt>Affected Versions</dt><dd>{entry.affectedVersions}</dd></> : null}
              {entry.reference ? <><dt>Reference</dt><dd>{entry.reference}</dd></> : null}
              {entry.notes ? <><dt>Notes</dt><dd>{entry.notes}</dd></> : null}
            </dl>
          </li>)}</ul> : <p className="workspace-surface-details-muted">No individual advisories recorded.</p>}
          <DetailText label="Earlier Notes" value={surface.foundation?.bugHistory?.summary} />
          <DetailText label="Search Limits" value={surface.foundation?.bugHistory?.limits} />
        </section>
        <section className="workspace-surface-details-section workspace-surface-recon-list">
          <h3>Leads</h3>
          {surfaceLeads.length ? <ul>{surfaceLeads.map((lead) => <li key={lead.id}>
            {onOpenClaim ? <button className="workspace-surface-claim-link" type="button" onClick={() => onOpenClaim(lead.id)}>{lead.title}</button>
              : <span>{lead.title}</span>}
          </li>)}</ul> : <p className="workspace-surface-details-muted">No leads assigned to this surface.</p>}
        </section>
      </> : activeView === 'Analysis' ? <>
        <section className="workspace-surface-details-section">
          <h3>Findings</h3>
          {surfaceFindings.length ? <ul className="workspace-surface-analysis-list">{surfaceFindings.map(({ finding, assignedSurfaceId }) => {
            const rating = campaignClaimRatingPresentation(finding);
            return <li key={finding.id}>
              {onOpenClaim ? <button className="workspace-surface-claim-link" type="button" onClick={() => onOpenClaim(finding.id)}>{finding.title}</button>
                : <span>{finding.title}</span>}
              <small title={rating.title}>{rating.label} · {traceLabel(finding.maturity)}{assignedSurfaceId !== surface.id
                ? ` · ${surfaceNames.get(assignedSurfaceId) ?? 'Component'}` : ''}</small>
            </li>;
          })}</ul> : <p className="workspace-surface-details-muted">No findings recorded for this surface or its components.</p>}
        </section>
        {surface.coverageState.unblockedFindingIds.length ? <section className="workspace-surface-details-section">
          <h3>Findings Needing Resolution</h3><p>{surface.coverageState.unblockedFindingIds.length} recorded</p>
        </section> : null}
        <section className="workspace-surface-details-section">
          <h3>Trajectories</h3>
          {surfaceTrajectories.length ? <ul className="workspace-surface-analysis-list">{surfaceTrajectories.map((node) => <li key={node.id}>
            {onOpenMemory ? <button className="workspace-surface-claim-link" type="button" onClick={() => onOpenMemory(node.id)}>{node.title}</button>
              : <span>{node.title}</span>}
            <small>{traceLabel(node.status)}{node.surfaceId && node.surfaceId !== surface.id
              ? ` · ${surfaceNames.get(node.surfaceId) ?? 'Component'}` : ''}</small>
          </li>)}</ul> : <p className="workspace-surface-details-muted">No trajectories recorded for this surface or its components.</p>}
        </section>
      </> : activeView === 'Threat Model' ? <>
        <section className="workspace-surface-details-section" aria-label="Threat model diagram">
          <SurfaceThreatDiagram source={surface.foundation?.threatModel?.diagram ?? ''} surfaceName={surface.name} />
        </section>
        <ThreatTextList title="Security Objectives" value={surface.foundation?.threatModel?.securityObjectives} />
        <ThreatTextList title="Risks" value={surface.foundation?.threatModel?.risks
          ?? surface.foundation?.threatModel?.riskImpact ?? surface.foundation?.threatModel?.plausibleImpacts} />
        <ThreatTextList title="Assumptions" value={surface.foundation?.threatModel?.assumptions} />
        <ThreatMemoryList title="Invariants" nodes={surfaceMemories.filter((node) => node.type === 'invariant')} onOpenMemory={onOpenMemory} />
        <ThreatMemoryList title="Sinks" nodes={surfaceMemories.filter((node) => node.type === 'sink')} onOpenMemory={onOpenMemory} />
        <ThreatMemoryList title="Mitigations" nodes={surfaceMemories.filter((node) => node.type === 'mitigation')} onOpenMemory={onOpenMemory} />
        {surface.foundation?.threatModel && (surface.foundation.threatModel.attackerControl
          || surface.foundation.threatModel.trustBoundaries || surface.foundation.threatModel.assetsAtRisk
          || surface.foundation.threatModel.invariants || surface.foundation.threatModel.sinks || surface.foundation.threatModel.mitigations)
          ? <section className="workspace-surface-details-section">
            <h3>Earlier Context</h3>
            <DetailText label="Attacker Control" value={surface.foundation.threatModel.attackerControl} />
            <DetailText label="Trust Boundaries" value={surface.foundation.threatModel.trustBoundaries} />
            <DetailText label="Assets At Risk" value={surface.foundation.threatModel.assetsAtRisk} />
            <DetailText label="Earlier Invariants" value={surface.foundation.threatModel.invariants} />
            <DetailText label="Earlier Sinks" value={surface.foundation.threatModel.sinks} />
            <DetailText label="Earlier Mitigations" value={surface.foundation.threatModel.mitigations} />
          </section> : null}
      </> : <div className="workspace-surface-resource">
        {resourceItem && !readOnly && onChangeResource ? <WorkspaceResourceEditor
          key={resourceItem.asset.id}
          presentation="sidebar"
          initialAsset={resourceItem.asset}
          initialClonedDirectory={resourceItem.repositoryLocalPath}
          kind={resourceItem.asset.kind}
          onEditStateChange={setResourceEditState}
          onClose={onClose}
          onRemove={() => onChangeResource(resourceItem.assetIds, null)}
          onSubmit={(asset) => onChangeResource(resourceItem.assetIds, asset)}
        /> : <section className="workspace-surface-details-section">
          <h3>Resource Details</h3>
          {surface.resourceKind ? <p><strong>Type</strong><span>{surface.resourceKind}</span></p> : null}
          {surface.resourceLocator ? <p><strong>Reference</strong><span>{surface.resourceLocator}</span></p> : null}
          {surface.resourceSource ? <p><strong>Source</strong><span>{surface.resourceSource.replaceAll('_', ' ')}</span></p> : null}
          {surface.resourceDirection ? <p><strong>Scope</strong><span>{surface.resourceDirection === 'in_scope' ? 'In scope' : surface.resourceDirection === 'out_of_scope' ? 'Out of scope' : 'Discovered'}</span></p> : null}
        </section>}
        {resourceItem?.repositoryCloned !== null && resourceItem?.repositoryCloned !== undefined ? <section className="workspace-surface-details-section">
          <h3>Repository Checkout</h3>
          <p>{resourceItem.repositoryCloned ? 'Cloned' : 'Not cloned'}</p>
          {resourceItem.repositoryLocalPath ? <p className="workspace-surface-details-description">{resourceItem.repositoryLocalPath}</p> : null}
          {!readOnly && onCloneRepository && resourceItem.asset.direction === 'in_scope' && !resourceItem.repositoryCloned && resourceItem.repositoryCloneAssetId
            ? <button className="secondary-button workspace-surface-resource-clone" type="button" onClick={() => setCloneMode('deep')}>
              <Download aria-hidden="true" size={14} />Clone repository</button> : null}
          {cloneError ? <p className="form-error" role="alert">{cloneError}</p> : null}
        </section> : null}
        {resourceItem && workspaceId && !readOnly ? <ResourcePriorArtView key={resourceItem.asset.id}
          workspaceId={workspaceId} assetIds={resourceItem.assetIds} /> : null}
      </div>}
    </div>
    {pendingNavigation ? <Modal title="Discard resource changes?" onClose={() => setPendingNavigation(null)} footer={<>
      <button className="secondary-button" type="button" onClick={() => setPendingNavigation(null)}>Keep editing</button>
      <button className="primary-button" type="button" onClick={() => {
        const destination = pendingNavigation;
        setPendingNavigation(null);
        setResourceEditState({ dirty: false, pending: false });
        if (destination === 'back') onClose();
        else setSelection({ surfaceId: surface.id, view: destination });
      }}>Discard changes</button>
    </>}><p>Your unsaved resource edits will be discarded.</p></Modal> : null}
    {cloneMode && resourceItem?.repositoryCloneAssetId && onCloneRepository ? <Modal title="Clone repository"
      onClose={() => setCloneMode(null)} footer={<>
        <button className="secondary-button" type="button" disabled={clonePending} onClick={() => setCloneMode(null)}>Cancel</button>
        <button className="primary-button" type="button" disabled={clonePending} onClick={() => {
          setClonePending(true);
          setCloneError(null);
          void onCloneRepository(resourceItem.repositoryCloneAssetId!, cloneMode).then(() => setCloneMode(null),
            (caught: unknown) => setCloneError(errorMessage(caught))).finally(() => setClonePending(false));
        }}>{clonePending ? 'Cloning…' : 'Clone repository'}</button>
      </>}>
      <p>Choose how much Git history to keep for {resourceItem.label}.</p>
      <div className="settings-form-radio-list workspace-repository-clone-options">
        {(['deep', 'shallow'] as const).map((mode) => <label className="settings-form-control-row" key={mode}>
          <span className="settings-form-radio-copy"><strong>{mode === 'deep' ? 'Deep clone' : 'Shallow clone'}</strong>
            <small>{mode === 'deep' ? 'Complete history for research and prior-fix analysis.' : 'Current history tip only.'}</small></span>
          <input type="radio" name="surface-repository-clone-mode" value={mode} checked={cloneMode === mode}
            onChange={() => setCloneMode(mode)} />
        </label>)}
      </div>
      {cloneError ? <p className="form-error" role="alert">{cloneError}</p> : null}
    </Modal> : null}
  </aside>;
}
