import { useCallback, useEffect, useRef, useState } from 'react';
import type { CSSProperties, JSX, RefObject } from 'react';
import { BadgeCheck, CircleHelp, Eye, FlaskConical, GitBranch, Lightbulb, Minus, Plus } from 'lucide-react';
import type {
  HoneycrispFindingSummary,
  HoneycrispMemorySummary,
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
import { memoryTypeClassName, memoryTypeLabel, memoryTypeStyle } from '../research/MemoryTypeLabel';

const CAMPAIGN_PRIORITY_CLAIM_LIMIT = 8;
const CAMPAIGN_BOARD_MATURITIES = ['refuted', 'observed', 'reproduced', 'verified'] as const;
type CampaignBoardMaturity = typeof CAMPAIGN_BOARD_MATURITIES[number];
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
const CAMPAIGN_CLAIM_RATING_RANK: Readonly<Record<HoneycrispFindingSummary['rating'], number>> = {
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
  onOpenClaim,
  onOpenRunbook
}: {
  memory: HoneycrispMemorySummary | null;
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
  workspaceName: string;
  onOpenClaim: (claimId: string) => void;
  onOpenRunbook: (runbookId: string) => void;
}): JSX.Element {
  const campaign = memory?.campaign;
  const priorityClaims = campaignPriorityClaims(memory);
  const loading = memory === null || memory.loading === true;
  const priorityScrollFades = useCampaignScrollFades(
    'horizontal',
    priorityClaims.map((claim) => `${claim.id}:${claim.revision}`).join('|')
  );
  const trailScrollFades = useCampaignScrollFades(
    'vertical',
    (campaign?.tracks ?? []).map((track) => `${track.id}:${track.revision}:${track.questions.length}:${track.experiments.length}:${track.observations.length}`).join('|')
  );

  return (
    <section aria-labelledby="workspace-campaign-heading" className="workspace-dashboard-panel campaign-panel" id="workspace-dashboard-campaign-trail-panel" role="tabpanel">
      <header className="campaign-header">
        <div className="settings-form-heading campaign-view-heading">
          <h2 className="campaign-view-title" id="workspace-campaign-heading">{workspaceName.trim() || 'Workspace'} Trail</h2>
          <p>{loading ? 'Loading campaign…' : campaign?.momentum.reason ?? 'No campaign context available.'}</p>
        </div>
      </header>

      <div className="campaign-trail-layout">
        <section className="campaign-trail-section campaign-priority-claims" aria-labelledby="campaign-priority-claims-heading">
          <h3 className="campaign-trail-section-heading" id="campaign-priority-claims-heading">Priority Claims</h3>
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

        <section className="campaign-trail-section campaign-trail-hierarchy" aria-labelledby="campaign-trail-hierarchy-heading">
          <h3 className="campaign-trail-section-heading" id="campaign-trail-hierarchy-heading">Campaign Trail</h3>
          <div className="campaign-trail-scroll-frame" ref={trailScrollFades.frameRef}>
            <div className="campaign-trail-scroll" onScroll={trailScrollFades.update} ref={trailScrollFades.scrollRef}>
              <CampaignTrailHierarchy
                activeTrackId={campaign?.activeTrackId ?? null}
                memory={memory}
                onOpenRunbook={onOpenRunbook}
              />
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
  onOpenClaim
}: {
  memory: HoneycrispMemorySummary | null;
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
  workspaceName: string;
  onOpenClaim: (claimId: string) => void;
}): JSX.Element {
  const loading = memory === null || memory.loading === true;
  const [classificationFilter, setClassificationFilter] = useState('all');
  const [ratingFilter, setRatingFilter] = useState<CampaignBoardRatingFilter>('all');
  const classificationOptions = campaignBoardClassificationOptions(memory);

  useEffect(() => {
    if (classificationFilter === 'all' || classificationOptions.some((option) => option.value === classificationFilter)) return;
    setClassificationFilter('all');
  }, [classificationFilter, classificationOptions]);

  return (
    <section aria-labelledby="workspace-campaign-board-heading" className="workspace-dashboard-panel campaign-panel campaign-board-panel" id="workspace-dashboard-campaign-board-panel" role="tabpanel">
      <header className="campaign-header campaign-board-header">
        <div className="settings-form-heading campaign-view-heading">
          <h2 className="campaign-view-title" id="workspace-campaign-board-heading">{workspaceName.trim() || 'Workspace'} Board</h2>
          <p>Findings grouped by maturity; proposed leads are excluded.</p>
        </div>
        <div className="campaign-board-filters" aria-label="Board filters">
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

      <div className="campaign-board-lanes">
        {CAMPAIGN_BOARD_MATURITIES.map((maturity) => {
          const findings = campaignBoardFindings(memory, maturity, { classification: classificationFilter, rating: ratingFilter });
          return (
            <section className={`campaign-board-lane maturity-${maturity}`} key={maturity} aria-labelledby={`campaign-board-${maturity}-heading`}>
              <h3 className="campaign-trail-section-heading campaign-board-lane-heading" id={`campaign-board-${maturity}-heading`}>{traceLabel(maturity)} ({findings.length.toLocaleString()})</h3>
              <div className="campaign-board-lane-list">
                {loading ? <p className="campaign-trail-section-empty">Loading findings.</p> : findings.length === 0 ? (
                  <p className="campaign-trail-section-empty">No {maturity} findings.</p>
                ) : findings.map((claim) => (
                  <CampaignClaimCard
                    claim={claim}
                    className="campaign-board-card"
                    key={claim.id}
                    metadata={campaignBoardClaimMetadata(claim)}
                    onOpenClaim={onOpenClaim}
                    providerModelCatalog={providerModelCatalog}
                  />
                ))}
              </div>
            </section>
          );
        })}
      </div>
    </section>
  );
}

type CampaignScrollAxis = 'horizontal' | 'vertical';

function useCampaignScrollFades(axis: CampaignScrollAxis, contentKey: string): {
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
      scrollSize: axis === 'horizontal' ? scroll.scrollWidth : scroll.scrollHeight,
      clientSize: axis === 'horizontal' ? scroll.clientWidth : scroll.clientHeight,
      scrollOffset: axis === 'horizontal' ? scroll.scrollLeft : scroll.scrollTop
    });
    const leadingClass = axis === 'horizontal' ? 'has-left-fade' : 'has-top-fade';
    const trailingClass = axis === 'horizontal' ? 'has-right-fade' : 'has-bottom-fade';
    frame.classList.toggle(leadingClass, edges.leading);
    frame.classList.toggle(trailingClass, edges.trailing);
  }, [axis]);

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
  authors: HoneycrispFindingSummary['authors'];
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

export function campaignPriorityClaimMetadata(claim: HoneycrispFindingSummary): string {
  return `${traceLabel(claim.projection)} ${traceLabel(claim.maturity)}, ${traceLabel(claim.rating)} ${campaignBoardClassificationLabel(claim.classification)}`;
}

export function campaignBoardClaimMetadata(claim: HoneycrispFindingSummary): string {
  return campaignClaimRatingPresentation(claim).label;
}

export function campaignBoardFindings(
  memory: HoneycrispMemorySummary | null,
  maturity: CampaignBoardMaturity,
  filters: { classification: string; rating: CampaignBoardRatingFilter } = { classification: 'all', rating: 'all' }
): HoneycrispFindingSummary[] {
  return (memory?.findings ?? []).filter((claim) => claim.maturity === maturity
    && (filters.classification === 'all' || claim.classification === filters.classification)
    && (filters.rating === 'all' || campaignClaimRatingPresentation(claim).value === filters.rating));
}

export function campaignBoardClassificationOptions(memory: HoneycrispMemorySummary | null): Array<{ value: string; label: string }> {
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

export function campaignPriorityClaims(memory: HoneycrispMemorySummary | null): HoneycrispFindingSummary[] {
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
  providerModelCatalog
}: {
  claim: HoneycrispFindingSummary;
  className?: string;
  metadata: string;
  onOpenClaim: (claimId: string) => void;
  providerModelCatalog: readonly ResearchProviderModelCatalog[];
}): JSX.Element {
  return (
    <button className={`campaign-priority-claim${className ? ` ${className}` : ''}`} onClick={() => onOpenClaim(claim.id)} type="button">
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

function CampaignTrailHierarchy({
  activeTrackId,
  memory,
  onOpenRunbook
}: {
  activeTrackId: string | null;
  memory: HoneycrispMemorySummary | null;
  onOpenRunbook: (runbookId: string) => void;
}): JSX.Element {
  const tracks = memory?.campaign.tracks ?? [];

  return (
    <section className="campaign-trail-tree" aria-label="Campaign trail">
      {tracks.length === 0 ? <div className="campaign-trail-empty">No campaign tracks yet.</div> : tracks.map((track, trackIndex) => (
        <details
          className={track.id === activeTrackId ? 'campaign-tree-track active' : 'campaign-tree-track'}
          key={track.id}
          open={track.id === activeTrackId || (activeTrackId === null && trackIndex === 0)}
        >
          <summary className="campaign-tree-summary campaign-tree-track-summary">
            <CampaignTreeToggle />
            <span className="campaign-tree-item-copy campaign-tree-branch-copy">
              <GitBranch aria-hidden="true" className="campaign-tree-item-icon" size={13} />
              <span className="campaign-tree-item-type">investigation</span>
              <span className="campaign-tree-item-name">{track.title}</span>
            </span>
            <span className="campaign-tree-summary-meta"><span>{track.counts.experiments} {track.counts.experiments === 1 ? 'experiment' : 'experiments'}</span><span>{track.stage}</span><span>{track.status}</span></span>
          </summary>
          <div className="campaign-tree-children">
            <CampaignTrackChildren track={track} onOpenRunbook={onOpenRunbook} />
          </div>
        </details>
      ))}
    </section>
  );
}

type CampaignTrack = NonNullable<HoneycrispMemorySummary['campaign']['tracks']>[number];
type CampaignQuestion = CampaignTrack['questions'][number];
type CampaignExperiment = CampaignTrack['experiments'][number];
type CampaignObservation = CampaignTrack['observations'][number];

function CampaignTrackChildren({
  track,
  onOpenRunbook
}: {
  track: CampaignTrack;
  onOpenRunbook: (runbookId: string) => void;
}): JSX.Element {
  const knownQuestionIds = new Set(track.questions.map((question) => question.id));
  const knownExperimentIds = new Set(track.experiments.map((experiment) => experiment.id));
  const unlinkedExperiments = track.experiments.filter((experiment) => !experiment.questionId || !knownQuestionIds.has(experiment.questionId));
  const unlinkedObservations = track.observations.filter((observation) => !observation.experimentId || !knownExperimentIds.has(observation.experimentId));
  const hasChildren = track.questions.length > 0 || unlinkedExperiments.length > 0 || unlinkedObservations.length > 0;

  if (!hasChildren) return <div className="campaign-tree-empty campaign-tree-node">No questions, experiments, or observations in this investigation.</div>;
  return (
    <>
      {track.questions.map((question) => (
        <CampaignQuestionNode
          experiments={track.experiments.filter((experiment) => experiment.questionId === question.id)}
          key={question.id}
          observations={track.observations}
          onOpenRunbook={onOpenRunbook}
          question={question}
        />
      ))}
      {unlinkedExperiments.map((experiment) => (
        <CampaignExperimentNode
          experiment={experiment}
          key={experiment.id}
          observations={track.observations.filter((observation) => observation.experimentId === experiment.id)}
          onOpenRunbook={onOpenRunbook}
        />
      ))}
      {unlinkedObservations.map((observation) => <CampaignObservationRow key={observation.id} observation={observation} />)}
    </>
  );
}

function CampaignQuestionNode({
  experiments,
  observations,
  onOpenRunbook,
  question
}: {
  experiments: CampaignExperiment[];
  observations: CampaignObservation[];
  onOpenRunbook: (runbookId: string) => void;
  question: CampaignQuestion;
}): JSX.Element {
  return (
    <details className="campaign-tree-question campaign-tree-node" open>
      <summary className="campaign-tree-summary campaign-tree-question-summary">
        <CampaignTreeToggle />
        <span className="campaign-tree-item-copy campaign-tree-branch-copy">
          <CircleHelp aria-hidden="true" className="campaign-tree-item-icon" size={13} />
          <span className="campaign-tree-item-type">question</span>
          <span className="campaign-tree-item-name">{question.text}</span>
        </span>
        <span className="campaign-tree-summary-meta"><span>{question.priority}</span><span>{question.status}</span></span>
      </summary>
      <div className="campaign-tree-children">
        {experiments.length === 0
          ? <div className="campaign-tree-empty campaign-tree-node">No experiments for this question.</div>
          : experiments.map((experiment) => (
              <CampaignExperimentNode
                experiment={experiment}
                key={experiment.id}
                observations={observations.filter((observation) => observation.experimentId === experiment.id)}
                onOpenRunbook={onOpenRunbook}
              />
            ))}
      </div>
    </details>
  );
}

function CampaignExperimentNode({
  experiment,
  observations,
  onOpenRunbook
}: {
  experiment: CampaignExperiment;
  observations: CampaignObservation[];
  onOpenRunbook: (runbookId: string) => void;
}): JSX.Element {
  return (
    <div className="campaign-tree-experiment-branch campaign-tree-node">
      <CampaignExperimentRow experiment={experiment} onOpenRunbook={onOpenRunbook} />
      {observations.length > 0 ? (
        <div className="campaign-tree-children">
          {observations.map((observation) => <CampaignObservationRow key={observation.id} observation={observation} />)}
        </div>
      ) : null}
    </div>
  );
}

function CampaignTreeToggle(): JSX.Element {
  return <span aria-hidden="true" className="campaign-tree-toggle"><Plus className="campaign-tree-toggle-collapsed" size={12} /><Minus className="campaign-tree-toggle-expanded" size={12} /></span>;
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

function CampaignExperimentRow({
  experiment,
  onOpenRunbook
}: {
  experiment: NonNullable<NonNullable<HoneycrispMemorySummary['campaign']['tracks']>[number]['experiments']>[number];
  onOpenRunbook: (runbookId: string) => void;
}): JSX.Element {
  const content = <><FlaskConical aria-hidden="true" className="campaign-tree-item-icon" size={13} /><span className="campaign-tree-item-copy"><span className="campaign-tree-item-type">experiment</span><span className="campaign-tree-item-name">{experiment.title}</span></span><span className="campaign-tree-item-status">{experiment.status.replaceAll('_', ' ')}</span></>;
  return experiment.runbookId
    ? <button className="campaign-tree-item campaign-tree-item-experiment" onClick={() => onOpenRunbook(experiment.runbookId!)} title={experiment.resultSummary || experiment.title} type="button">{content}</button>
    : <div className="campaign-tree-item campaign-tree-item-experiment" title={experiment.resultSummary || experiment.title}>{content}</div>;
}

function CampaignObservationRow({ observation }: { observation: CampaignObservation }): JSX.Element {
  return (
    <div className="campaign-tree-item campaign-tree-item-observation campaign-tree-node" title={`${observation.kind} observation`}>
      <Eye aria-hidden="true" className="campaign-tree-item-icon" size={13} />
      <span className="campaign-tree-item-copy">
        <span className="campaign-tree-item-type">observation</span>
        <span className="campaign-tree-item-name">{observation.summary}</span>
      </span>
      <span className="campaign-tree-item-status">{observation.outcome}</span>
    </div>
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
