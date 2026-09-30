import { useCallback, useEffect, useRef, useState } from 'react';
import type { CSSProperties, DragEvent, JSX, RefObject } from 'react';
import { BadgeCheck, Lightbulb, Search } from 'lucide-react';
import type {
  AppServerFindingSummary,
  AppServerMemorySummary,
  ClaimBoardMaturity,
  ResearchProfileMemoryType,
  ResearchProviderModelCatalog,
  ResearchClaimRating
} from '@shared/types';
import { formatWorkspaceTimelineDuration } from '../../view-models/workspaceTimeline';
import type { SessionTimelineProjection } from '../../view-models/workspaceTimeline';
import type { SessionHeatPreferences } from '../../view-models/sessionHeat';
import { campaignClaimIsActive, campaignClaimRatingPresentation } from '../../view-models/campaignClaims';
import type { CampaignClaimRatingValue } from '../../view-models/campaignClaims';
import { researchModelDisplayName, traceLabel } from '../../lib/formatting';
import { ProviderIcon } from '../../app/ProviderIcon';
import { FloatingTextPicker } from '../../app/FloatingTextPicker';
import { MainSideScrollRegion } from '../../app/MainSideScrollRegion';
import { memoryTypeClassName, memoryTypeLabel, memoryTypeStyle } from '../research/MemoryTypeLabel';

const CAMPAIGN_PRIORITY_CLAIM_LIMIT = 8;
const CAMPAIGN_BOARD_MATURITIES: readonly ClaimBoardMaturity[] = ['refuted', 'observed', 'reproduced', 'verified'];
type CampaignBoardMaturity = ClaimBoardMaturity;
type CampaignBoardRatingFilter = CampaignClaimRatingValue | 'all';
const CAMPAIGN_BOARD_RATING_OPTIONS: Array<{ value: CampaignBoardRatingFilter; label: string }> = [
  { value: 'all', label: 'All Ratings' },
  { value: 'none', label: 'None' },
  { value: 'informational', label: 'Informational' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Medium' },
  { value: 'high', label: 'High' },
  { value: 'critical', label: 'Critical' }
];
const CAMPAIGN_CLAIM_RATING_RANK: Readonly<Record<AppServerFindingSummary['rating'], number>> = {
  critical: 0,
  high: 1,
  medium: 2,
  low: 3,
  informational: 4
};

export function CampaignGraphView({
  memory,
  providerModelCatalog,
  workspaceName,
  onOpenClaim
}: {
  memory: AppServerMemorySummary | null;
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
  workspaceName: string;
  onOpenClaim: (claimId: string) => void;
}): JSX.Element {
  const campaign = memory?.campaign;
  const priorityClaims = campaignPriorityClaims(memory);
  const loading = memory === null || memory.loading === true;
  const priorityScrollFades = useCampaignScrollFades(
    priorityClaims.map((claim) => `${claim.id}:${claim.revision}`).join('|')
  );

  return (
    <section aria-labelledby="workspace-campaign-heading" className="workspace-dashboard-panel campaign-panel" id="workspace-dashboard-campaign-trail-panel" role="tabpanel">
      <header className="campaign-header">
        <div className="settings-form-heading campaign-view-heading">
          <h2 className="campaign-view-title" id="workspace-campaign-heading">{workspaceName.trim() || 'Workspace'} Highlights</h2>
          <p>{loading ? 'Loading campaign…' : campaign?.momentum.reason ?? 'No campaign context available.'}</p>
        </div>
      </header>

      <div className="campaign-trail-layout">
        <section className="campaign-trail-section campaign-priority-claims" aria-labelledby="campaign-priority-claims-heading">
          <h3 className="workspace-campaign-list-heading" id="campaign-priority-claims-heading">Priority Claims</h3>
          <div className="campaign-priority-claim-scroll" ref={priorityScrollFades.frameRef}>
            <div className="campaign-priority-claim-list" onScroll={priorityScrollFades.update} ref={priorityScrollFades.scrollRef}>
              {loading ? <p className="campaign-trail-section-empty">Loading priority claims.</p> : priorityClaims.length === 0 ? (
                <p className="campaign-trail-section-empty">No active claims yet.</p>
              ) : priorityClaims.map((claim) => (
                <CampaignClaimCard
                  claim={claim}
                  key={claim.id}
                  metadata={campaignPriorityClaimMetadata(claim)}
                  onOpenClaim={onOpenClaim}
                  providerModelCatalog={providerModelCatalog}
                />
              ))}
            </div>
          </div>
        </section>
      </div>
    </section>
  );
}

export function CampaignBoardView({
  memory,
  providerModelCatalog,
  workspaceName,
  onOpenClaim,
  onTransitionClaim
}: {
  memory: AppServerMemorySummary | null;
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
  workspaceName: string;
  onOpenClaim: (claimId: string) => void;
  onTransitionClaim?: (claim: AppServerFindingSummary, targetMaturity: ClaimBoardMaturity) => Promise<void>;
}): JSX.Element {
  const loading = memory === null || memory.loading === true;
  const [query, setQuery] = useState('');
  const [classificationFilter, setClassificationFilter] = useState('all');
  const [ratingFilter, setRatingFilter] = useState<CampaignBoardRatingFilter>('all');
  const [draggedClaimId, setDraggedClaimId] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<ClaimBoardMaturity | null>(null);
  const [pendingClaimId, setPendingClaimId] = useState<string | null>(null);
  const [transitionError, setTransitionError] = useState<string | null>(null);
  const classificationOptions = campaignBoardClassificationOptions(memory);

  const dragClaim = (event: DragEvent<HTMLButtonElement>, claim: AppServerFindingSummary): void => {
    if (pendingClaimId || !onTransitionClaim) {
      event.preventDefault();
      return;
    }
    event.dataTransfer.effectAllowed = 'move';
    event.dataTransfer.setData('application/x-beale-claim-board', 'claim');
    setDraggedClaimId(claim.id);
    setDropTarget(null);
    setTransitionError(null);
  };

  const clearDrag = (): void => {
    setDraggedClaimId(null);
    setDropTarget(null);
  };

  const draggedClaim = memory?.findings.find((claim) => claim.id === draggedClaimId) ?? null;

  useEffect(() => {
    if (classificationFilter === 'all' || classificationOptions.some((option) => option.value === classificationFilter)) return;
    setClassificationFilter('all');
  }, [classificationFilter, classificationOptions]);

  return (
    <section aria-labelledby="workspace-campaign-board-heading" className="workspace-dashboard-panel campaign-panel campaign-board-panel" id="workspace-dashboard-campaign-board-panel" role="tabpanel">
      <header className="campaign-header campaign-board-header">
        <div className="settings-form-heading campaign-view-heading">
          <h2 className="campaign-view-title" id="workspace-campaign-board-heading">{workspaceName.trim() || 'Workspace'} Claims</h2>
          <p>Drag findings between columns to change status. Human moves are recorded as overrides; proposed leads are excluded.</p>
          {pendingClaimId ? <p className="campaign-board-transition-pending" role="status">Updating claim status…</p> : null}
          {transitionError ? <p className="campaign-board-transition-error" role="alert">{transitionError}</p> : null}
        </div>
        <div className="campaign-board-filters" aria-label="Claims filters">
          <label className="campaign-board-search">
            <Search size={14} aria-hidden="true" />
            <input
              aria-label="Filter claims"
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Filter findings"
              type="search"
              value={query}
            />
          </label>
          <FloatingTextPicker
            ariaLabel="Finding class filter"
            className="campaign-board-filter campaign-board-class-filter"
            onChange={setClassificationFilter}
            options={classificationOptions}
            title="Filter findings by class"
            value={classificationFilter}
          />
          <FloatingTextPicker
            ariaLabel="Finding rating filter"
            className="campaign-board-filter campaign-board-rating-filter"
            onChange={(value) => setRatingFilter(value as CampaignBoardRatingFilter)}
            options={CAMPAIGN_BOARD_RATING_OPTIONS}
            title="Filter findings by preferred CVSS or fallback rating"
            value={ratingFilter}
          />
        </div>
      </header>

      <div className="campaign-board-lanes" aria-busy={pendingClaimId !== null}>
        {CAMPAIGN_BOARD_MATURITIES.map((maturity) => {
          const findings = campaignBoardFindings(memory, maturity, { classification: classificationFilter, rating: ratingFilter, query });
          return (
            <section
              className={`campaign-board-lane maturity-${maturity}${dropTarget === maturity ? ' is-drop-target' : ''}`}
              key={maturity}
              aria-labelledby={`campaign-board-${maturity}-heading`}
              onDragOver={(event) => {
                if (!draggedClaim || draggedClaim.maturity === maturity || pendingClaimId) return;
                event.preventDefault();
                event.dataTransfer.dropEffect = 'move';
                if (dropTarget !== maturity) setDropTarget(maturity);
              }}
              onDrop={(event) => {
                event.preventDefault();
                clearDrag();
                if (!draggedClaim || draggedClaim.maturity === maturity || !onTransitionClaim || pendingClaimId) return;
                setPendingClaimId(draggedClaim.id);
                setTransitionError(null);
                void onTransitionClaim(draggedClaim, maturity)
                  .catch((error: unknown) => setTransitionError(error instanceof Error ? error.message : String(error)))
                  .finally(() => setPendingClaimId(null));
              }}
            >
              <h3 className="workspace-campaign-list-heading campaign-board-lane-heading" id={`campaign-board-${maturity}-heading`}>{traceLabel(maturity)} ({findings.length.toLocaleString()})</h3>
              <MainSideScrollRegion
                className="campaign-board-lane-scroll"
                listClassName="campaign-board-lane-list"
                updateKey={`${loading}:${classificationFilter}:${ratingFilter}:${query}:${findings.map(({ id, revision }) => `${id}:${revision}`).join('|')}`}
              >
                {loading ? <p className="campaign-trail-section-empty">Loading findings.</p> : findings.length === 0 ? (
                  <p className="campaign-trail-section-empty">No {maturity} findings.</p>
                ) : findings.map((claim) => (
                  <CampaignClaimCard
                    claim={claim}
                    className="campaign-board-card"
                    key={claim.id}
                    metadata={campaignBoardClaimMetadata(claim)}
                    onOpenClaim={onOpenClaim}
                    onDragStart={onTransitionClaim ? (event) => dragClaim(event, claim) : undefined}
                    onDragEnd={clearDrag}
                    dragging={draggedClaimId === claim.id}
                    dragDisabled={pendingClaimId !== null}
                    providerModelCatalog={providerModelCatalog}
                  />
                ))}
              </MainSideScrollRegion>
            </section>
          );
        })}
      </div>
    </section>
  );
}

function useCampaignScrollFades(contentKey: string): {
  frameRef: RefObject<HTMLDivElement | null>;
  scrollRef: RefObject<HTMLDivElement | null>;
  update: () => void;
} {
  const frameRef = useRef<HTMLDivElement | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const update = useCallback((): void => {
    const frame = frameRef.current;
    const scroll = scrollRef.current;
    if (!frame || !scroll) return;
    const edges = campaignScrollFadeEdges({
      scrollSize: scroll.scrollWidth,
      clientSize: scroll.clientWidth,
      scrollOffset: scroll.scrollLeft
    });
    frame.classList.toggle('has-left-fade', edges.leading);
    frame.classList.toggle('has-right-fade', edges.trailing);
  }, []);

  useEffect(() => {
    const scroll = scrollRef.current;
    update();
    if (!scroll || typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(update);
    observer.observe(scroll);
    for (const child of scroll.children) observer.observe(child);
    return () => observer.disconnect();
  }, [contentKey, update]);

  return { frameRef, scrollRef, update };
}

export function campaignScrollFadeEdges({
  scrollSize,
  clientSize,
  scrollOffset
}: {
  scrollSize: number;
  clientSize: number;
  scrollOffset: number;
}): { leading: boolean; trailing: boolean } {
  const scrollableDistance = scrollSize - clientSize;
  const canScroll = scrollableDistance > 8;
  return {
    leading: canScroll && scrollOffset > 8,
    trailing: canScroll && scrollOffset < scrollableDistance - 8
  };
}

function CampaignPriorityClaimAuthors({
  authors,
  providerModelCatalog
}: {
  authors: AppServerFindingSummary['authors'];
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
}): JSX.Element | null {
  const containerRef = useRef<HTMLSpanElement | null>(null);
  const [hasOverflow, setHasOverflow] = useState(false);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return undefined;
    const update = (): void => setHasOverflow(campaignPriorityClaimHasOverflow(container.scrollWidth, container.clientWidth));
    update();
    if (typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(update);
    observer.observe(container);
    return () => observer.disconnect();
  }, [authors, providerModelCatalog]);

  if (authors.length === 0) return null;
  return (
    <span
      aria-label="Model authors"
      className={`campaign-priority-claim-authors${hasOverflow ? ' has-overflow' : ''}`}
      ref={containerRef}
    >
      <span className="campaign-priority-claim-author-list">
        {authors.map((author) => {
          const modelName = researchModelDisplayName(author.provider, author.model, providerModelCatalog);
          return (
            <span className="campaign-priority-claim-author" key={`${author.provider}\0${author.model}`} title={`${author.provider}/${modelName}`}>
              <ProviderIcon provider={author.provider || author.model} size={13} aria-hidden="true" />
              <span>{modelName}</span>
            </span>
          );
        })}
      </span>
    </span>
  );
}

export function campaignPriorityClaimHasOverflow(scrollWidth: number, clientWidth: number): boolean {
  return scrollWidth > clientWidth;
}

export function campaignPriorityClaimMetadata(claim: AppServerFindingSummary): string {
  return `${traceLabel(claim.maturity)} ${traceLabel(claim.rating)}`;
}

export function campaignBoardClaimMetadata(claim: AppServerFindingSummary): string {
  return campaignClaimRatingPresentation(claim).label;
}

export function campaignBoardFindings(
  memory: AppServerMemorySummary | null,
  maturity: CampaignBoardMaturity,
  filters: { classification: string; rating: CampaignBoardRatingFilter; query?: string } = { classification: 'all', rating: 'all' }
): AppServerFindingSummary[] {
  const query = filters.query?.trim().toLocaleLowerCase() ?? '';
  return (memory?.findings ?? []).filter((claim) => claim.maturity === maturity
    && (filters.classification === 'all' || claim.classification === filters.classification)
    && (filters.rating === 'all' || campaignClaimRatingPresentation(claim).value === filters.rating)
    && (!query || [claim.id, claim.title, claim.summary, claim.impact, claim.classification, claim.status,
      claim.maturity, claim.workflow, claim.rating, ...claim.evidence.map((evidence) => evidence.summary)]
      .join('\n').toLocaleLowerCase().includes(query)));
}

export function campaignBoardClassificationOptions(memory: AppServerMemorySummary | null): Array<{ value: string; label: string }> {
  const classifications = [...new Set((memory?.findings ?? []).map((claim) => claim.classification).filter(Boolean))];
  return [
    { value: 'all', label: 'All Classes' },
    ...classifications
      .map((classification) => ({ value: classification, label: campaignBoardClassificationLabel(classification) }))
      .sort((left, right) => left.label.localeCompare(right.label))
  ];
}

function campaignBoardClassificationLabel(classification: string): string {
  const unqualified = classification.trim().split('.').filter(Boolean).at(-1) ?? classification;
  return traceLabel(unqualified.replaceAll('-', '_'));
}

export function campaignPriorityClaims(memory: AppServerMemorySummary | null): AppServerFindingSummary[] {
  if (!memory) return [];
  const actionRanks = new Map<string, number>();
  memory.campaign.nextActions.forEach((action, actionIndex) => {
    action.relatedNodeIds.forEach((nodeId) => {
      if (!actionRanks.has(nodeId)) actionRanks.set(nodeId, actionIndex);
    });
  });
  return [...memory.findings, ...memory.leads]
    .filter(campaignClaimIsActive)
    .sort((left, right) => {
      const leftActionRank = actionRanks.get(`${left.projection}:${left.id}`) ?? Number.MAX_SAFE_INTEGER;
      const rightActionRank = actionRanks.get(`${right.projection}:${right.id}`) ?? Number.MAX_SAFE_INTEGER;
      return leftActionRank - rightActionRank
        || CAMPAIGN_CLAIM_RATING_RANK[left.rating] - CAMPAIGN_CLAIM_RATING_RANK[right.rating]
        || (left.projection === right.projection ? 0 : left.projection === 'finding' ? -1 : 1)
        || Date.parse(right.updatedAt) - Date.parse(left.updatedAt)
        || left.title.localeCompare(right.title);
    })
    .slice(0, CAMPAIGN_PRIORITY_CLAIM_LIMIT);
}

function CampaignClaimCard({
  claim,
  className = '',
  metadata,
  onOpenClaim,
  onDragStart,
  onDragEnd,
  dragging = false,
  dragDisabled = false,
  providerModelCatalog
}: {
  claim: AppServerFindingSummary;
  className?: string;
  metadata: string;
  onOpenClaim: (claimId: string) => void;
  onDragStart?: (event: DragEvent<HTMLButtonElement>) => void;
  onDragEnd?: () => void;
  dragging?: boolean;
  dragDisabled?: boolean;
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
}): JSX.Element {
  return (
    <button
      className={`campaign-priority-claim${className ? ` ${className}` : ''}${dragging ? ' is-dragging' : ''}`}
      draggable={Boolean(onDragStart) && !dragDisabled}
      onClick={() => onOpenClaim(claim.id)}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      type="button"
    >
      <strong className="campaign-priority-claim-title">
        {claim.projection === 'finding'
          ? <BadgeCheck aria-hidden="true" className={`campaign-claim-title-icon maturity-${claim.maturity}`} size={15} />
          : <Lightbulb aria-hidden="true" className={`campaign-claim-title-icon maturity-${claim.maturity}`} size={15} />}
        <span>{claim.title}</span>
      </strong>
      <span className="campaign-priority-claim-footer">
        <span className="campaign-priority-claim-meta">{metadata}</span>
        <CampaignPriorityClaimAuthors authors={claim.authors} providerModelCatalog={providerModelCatalog} />
      </span>
    </button>
  );
}

export function CampaignSessionProjection({
  memoryTypes,
  profileId,
  projection,
  sessionHeatPreferences,
  sessionTitle
}: {
  memoryTypes: readonly ResearchProfileMemoryType[];
  profileId?: string;
  projection: SessionTimelineProjection | null;
  sessionHeatPreferences: SessionHeatPreferences;
  sessionTitle: string;
}): JSX.Element {
  const durationLabel = projection && projection.totalDurationMs > 0
    ? formatWorkspaceTimelineDuration(projection.totalDurationMs)
    : 'No activity recorded';
  return (
    <span
      aria-label={`${sessionTitle} complete session activity: ${durationLabel}`}
      className={`campaign-session-projection${projection?.segments.length ? '' : ' is-empty'}`}
      role="img"
      title={durationLabel}
    >
      {projection?.segments.map((segment) => (
        <span
          aria-hidden="true"
          className="workspace-timeline-segment"
          key={segment.id}
          style={{ left: `${segment.leftPercent}%`, width: `${segment.widthPercent}%` }}
          title={`${formatCampaignTimelineDateTime(segment.startedAt)} – ${segment.endedAt ? formatCampaignTimelineDateTime(segment.endedAt) : 'Now'}`}
        />
      ))}
      {projection?.memoryMarkers.map((marker) => (
        <span
          aria-hidden="true"
          className={`workspace-timeline-memory-marker ${memoryTypeClassName(marker.type, memoryTypes)}`}
          key={marker.id}
          style={{
            left: `${marker.leftPercent}%`,
            ...memoryTypeStyle(marker.type, memoryTypes, marker.status, profileId, sessionHeatPreferences)
          } as CSSProperties}
          title={`${memoryTypeLabel(marker.type, memoryTypes)} · ${marker.title} · ${formatCampaignTimelineDateTime(marker.createdAt)}`}
        />
      ))}
      {projection?.runbookRevisionMarkers.map((marker) => (
        <span
          aria-hidden="true"
          className="workspace-timeline-runbook-marker"
          key={marker.id}
          style={{ left: `${marker.leftPercent}%` }}
          title={`Runbook · ${marker.title} · Update ${marker.revision} · ${formatCampaignTimelineDateTime(marker.createdAt)}`}
        />
      ))}
      {projection?.reportRevisionMarkers.map((marker) => (
        <span
          aria-hidden="true"
          className="workspace-timeline-report-marker"
          key={marker.id}
          style={{ left: `${marker.leftPercent}%` }}
          title={`Report · ${marker.title} · Update ${marker.revision} · ${formatCampaignTimelineDateTime(marker.createdAt)}`}
        />
      ))}
    </span>
  );
}

function formatCampaignTimelineDateTime(value: string): string {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit'
  }).format(timestamp);
}
