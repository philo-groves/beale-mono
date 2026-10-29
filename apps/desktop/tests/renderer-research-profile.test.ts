import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type {
  AppServerFindingSummary,
  AppServerMemoryNodeSummary,
  ResearchProfile,
  ResearchProfileWorkflow,
  RunDetail,
  WorkspaceSnapshot
} from '@shared/types';
import { MemoryTypeLabel } from '../src/renderer/features/research/MemoryTypeLabel';
import {
  browserBaseUrl,
  campaignCatalogSectionVisibility,
  campaignEntryFilterSelection,
  campaignEntryScrollTargetSelector,
  CampaignClaimDetailView,
  CampaignClaimCatalogLists,
  NO_CAMPAIGN_MEMORIES_FILTER,
  orderedCatalogMemoryTypes,
  ResearchSideNestedHeader,
  ResearchSidePanel
} from '../src/renderer/features/research/MemorySidePanel';
import {
  defaultResearchWorkflowId,
  ResearchGoalChooser,
  StartRunForm
} from '../src/renderer/features/sessions/StartRunForm';
import { AppearanceSettingsView } from '../src/renderer/features/settings/SettingsModal';
import { formatSessionDateTime } from '../src/renderer/lib/formatting';
import {
  activeMemoryCount,
  memoryCatalogStatusSections,
  sessionMemoryActivitySummary,
  sessionMemoryTypeSummaries
} from '../src/renderer/view-models/memoryCatalog';
import { researchGoalSuggestionCacheKey } from '../src/renderer/view-models/researchGoalSuggestions';
import { testResearchProfile } from './researchProfileFixture';

describe('renderer research profile presentation', () => {
  it('shows the current browser host without a protocol as the summary label', () => {
    expect(browserBaseUrl('https://example.test:8443/path?q=1')).toBe('example.test:8443');
    expect(browserBaseUrl('http://example.test/path')).toBe('example.test');
    expect(browserBaseUrl('about:blank')).toBe('about:blank');
  });

  it('uses accessible circular back buttons for nested details', () => {
    const campaign = renderToStaticMarkup(createElement(ResearchSideNestedHeader, {
      iconOnly: true, label: 'Campaign', name: 'Example finding title', onBack: () => undefined
    }));
    const subagent = renderToStaticMarkup(createElement(ResearchSideNestedHeader, {
      iconOnly: true, label: 'Subagents', name: 'Example subagent',
      secondary: createElement('span', null, 'Example model'), onBack: () => undefined
    }));

    expect(campaign).toContain('aria-label="Back to Campaign"');
    expect(campaign).toContain('research-side-nested-header is-campaign-item');
    expect(campaign).toContain('class="is-icon-only"');
    expect(campaign).not.toContain('<span>Back to Campaign</span>');
    expect(campaign).toContain('class="research-side-nested-name" title="Example finding title">Example finding title</span>');
    expect(subagent).toContain('aria-label="Back to Subagents"');
    expect(subagent).toContain('class="is-icon-only"');
    expect(subagent).not.toContain('<span>Back to Subagents</span>');
    expect(subagent).toContain('title="Example subagent">Example subagent</span>');
    expect(subagent).toContain('research-side-nested-secondary');
    expect(subagent).toContain('Example model');
  });

  it('uses canonical session memory activity instead of the current trace projection', () => {
    expect(sessionMemoryActivitySummary([], 0, {
      memorySearches: 37,
      memoryUpdates: 12
    })).toBe('37 Searches, 12 Updates');
  });

  it('renders arbitrary workflows and bounded profile-sized loading states', () => {
    const workflows: ResearchProfileWorkflow[] = [
      workflow('survey', 'Survey', 2, true),
      workflow('synthesize', 'Synthesize', 20)
    ];
    const html = renderToStaticMarkup(createElement(ResearchGoalChooser, {
      workflows,
      suggestions: {
        survey: ['Map the corpus.', 'Identify missing sources.'],
        synthesize: ['Compare the strongest explanations.']
      },
      loading: { survey: false, synthesize: true },
      errors: { survey: null, synthesize: null },
      selectedWorkflowId: 'synthesize',
      onSelect: () => undefined,
      onRetry: () => undefined
    }));

    expect(defaultResearchWorkflowId(workflows)).toBe('survey');
    expect(html).toContain('>Survey</button>');
    expect(html).toContain('>Synthesize</button>');
    expect(html).toContain('Synthesize the available material.');
    expect(html).not.toContain('Synthesize suggestions');
    expect(html).not.toContain('Survey suggestions');
    expect(html).toContain('research-goal-choice-scroll');
    expect(html).not.toContain('Map the corpus.');
    expect(html).not.toContain('Discovery');
    expect(html.match(/research-goal-choice-loading/g)).toHaveLength(11);
  });

  it('uses presentation labels and the default suggestion lane in the start surface', () => {
    const profile = customProfile();
    const html = renderToStaticMarkup(createElement(StartRunForm, {
      snapshot: snapshot(profile),
      openAiStatus: null,
      defaultProviderId: 'openai-codex',
      providerModelDefaults: {},
      researchProviderStatuses: [],
      providerModelCatalog: [],
      researchGoalSuggestions: { survey: ['Map the corpus.'], synthesize: [] },
      researchGoalSuggestionsLoading: { survey: false, synthesize: false },
      researchGoalSuggestionErrors: { survey: null, synthesize: null },
      busy: false,
      runAction: async () => undefined,
      onCancel: () => undefined,
      onRetryResearchGoalSuggestions: () => undefined,
      onStarted: () => undefined
    }));

    expect(html).toContain('aria-label="Close New Inquiry"');
    expect(html).not.toContain('Study Settings');
    expect(html).not.toContain('>Minutes<');
    expect(html).toContain('aria-label="Suggestion lanes"');
    expect(html).toContain('>Survey</button>');
    expect(html).toContain('>Start</button>');
  });

  it('orders exact profile statuses and retains unknown stored values', () => {
    const memory = customProfile().memory;
    const sections = memoryCatalogStatusSections([
      node('published', 'note', 'published'),
      node('draft', 'note', 'draft'),
      node('historical', 'retired_note', 'historical_state')
    ], memory.statuses);

    expect(sections.map((section) => [section.id, section.label, section.nodes.map((entry) => entry.id)])).toEqual([
      ['draft', 'Working', ['draft']],
      ['published', 'Published', ['published']],
      ['historical_state', 'Unknown status (Historical state)', ['historical']]
    ]);
  });

  it('uses profile type names, groups, heat colors, status labels, and readable fallbacks', () => {
    const profile = customProfile();
    const nodes = [
      node('published_note', 'note', 'published'),
      node('draft_note', 'note', 'draft'),
      node('retired', 'retired_note', 'draft'),
      node('unknown', 'old_observation', 'draft')
    ];

    expect(orderedCatalogMemoryTypes(nodes, profile.memory.types)).toEqual([
      { id: 'retired_note', label: 'Archived Note', group: 'Archive' },
      { id: 'note', label: 'Note', group: 'Knowledge' },
      { id: 'old_observation', label: 'Unknown type (Old observation)' }
    ]);
    expect(sessionMemoryTypeSummaries(nodes, profile.memory).map((summary) => ({
      type: summary.type,
      countLabel: summary.countLabel,
      statusLabel: summary.statusLabel
    }))).toEqual([
      { type: 'retired_note', countLabel: '1 Archived Note', statusLabel: '1 Working' },
      { type: 'note', countLabel: '2 Notes', statusLabel: '1 Working, 1 Published' },
      { type: 'old_observation', countLabel: '1 Unknown type (Old observation)', statusLabel: '1 Working' }
    ]);

    const retiredHtml = renderToStaticMarkup(createElement(MemoryTypeLabel, {
      type: 'retired_note',
      definitions: profile.memory.types
    }));
    expect(retiredHtml).toContain('data-memory-type-lifecycle="retired"');
    expect(retiredHtml).not.toContain('--memory-type-color');
    expect(retiredHtml).toContain('>Archived Note</span>');

    const draftHtml = renderToStaticMarkup(createElement(MemoryTypeLabel, {
      type: 'note',
      definitions: profile.memory.types,
      status: 'draft'
    }));
    expect(draftHtml).not.toContain('data-memory-heat');
    expect(draftHtml).not.toContain('--memory-type-color');

    const publishedHtml = renderToStaticMarkup(createElement(MemoryTypeLabel, {
      type: 'note',
      definitions: profile.memory.types,
      status: 'published'
    }));
    expect(publishedHtml).toContain('data-memory-heat="high"');
    expect(publishedHtml).toContain('style="--memory-type-color:var(--session-heat-high-color)"');

    const overriddenHtml = renderToStaticMarkup(createElement(MemoryTypeLabel, {
      type: 'note',
      definitions: profile.memory.types,
      status: 'published',
      profileId: profile.id,
      sessionHeatPreferences: {
        heatOverrides: { [profile.id]: { note: { published: 'critical' } } },
        paletteOverrides: {}
      }
    }));
    expect(overriddenHtml).toContain('data-memory-heat="critical"');
    expect(overriddenHtml).toContain('style="--memory-type-color:var(--session-heat-critical-color)"');

    const unknownHtml = renderToStaticMarkup(createElement(MemoryTypeLabel, {
      type: 'old_observation',
      definitions: profile.memory.types
    }));
    expect(unknownHtml).toContain('Unknown type (Old observation)');
  });

  it('lists observed memory types separately when no profile is available', () => {
    const summaries = sessionMemoryTypeSummaries([
      node('note_one', 'research_note', 'confirmed'),
      node('note_two', 'research_note', 'suspected'),
      node('lead_one', 'lead', 'confirmed'),
      node('trajectory_one', 'trajectory', 'confirmed'),
      node('trajectory_two', 'trajectory', 'suspected'),
      node('rejected_trajectory', 'trajectory', 'rejected'),
      node('rejected_only', 'candidate', 'rejected'),
      node('stale_note', 'research_note', 'stale')
    ]);

    expect(summaries.map(({ type, countLabel, statusLabel }) => ({ type, countLabel, statusLabel }))).toEqual([
      { type: 'research_note', countLabel: '2 Research notes', statusLabel: '1 Confirmed, 1 Suspected' },
      { type: 'lead', countLabel: '1 Lead', statusLabel: '1 Confirmed' },
      { type: 'trajectory', countLabel: '2 Trajectories', statusLabel: '1 Confirmed, 1 Suspected' }
    ]);
    expect(sessionMemoryTypeSummaries([node('trajectory_one', 'trajectory', 'confirmed')])[0]?.countLabel)
      .toBe('1 Trajectory');
    expect(activeMemoryCount([
      node('trajectory_one', 'trajectory', 'confirmed'),
      node('rejected_trajectory', 'trajectory', 'rejected')
    ])).toBe(1);
  });

  it('includes profile workflow and vocabulary identity in suggestion cache keys', () => {
    const first = snapshot(customProfile());
    const second = snapshot({
      ...customProfile(),
      presentation: { ...customProfile().presentation, sessionLabel: 'Expedition' }
    });
    second.researchProfile.profileHash = first.researchProfile.profileHash;

    expect(researchGoalSuggestionCacheKey(first)).not.toBe(researchGoalSuggestionCacheKey(second));
    expect(researchGoalSuggestionCacheKey(first)).toContain('survey%3A2%2Csynthesize%3A3');
  });

  it('keeps sidenav resource and session labels canonical', () => {
    const profile = customProfile();
    const html = renderToStaticMarkup(createElement(ResearchSidePanel, {
      detail: null,
      events: [],
      memory: { nodes: [], runbooks: [], edges: [], contextWorkspaceId: 'w', contextSubjectId: 's' } as never,
      researchProfile: profile,
      providerModelCatalog: [],
      runId: 'run_one',
      runStatus: null,
      selectedRunbook: null,
      selectedRunbookDocument: null,
      runbookLoading: false,
      runbookError: null,
      selectedSubagentPath: null,
      selectedRunbookId: null,
      searchHighlightQuery: '',
      onOpenRunbook: () => undefined,
      onSelectSubagent: () => undefined,
      onBackToRunbooks: () => undefined,
      onBackToSubagents: () => undefined,
    }));
    expect(html).toContain('aria-label="Session summary"');
    expect(html).toContain('>Session</h2>');
    expect(html).not.toContain('Study summary');
    expect(html).toContain('aria-label="Loading session memories"');
    expect(html).toContain('aria-label="Open Browser Default"');
    expect(html).toContain('<span title="about:blank">about:blank</span>');
    expect(html).toMatch(/aria-label="Session metadata"[\s\S]*?<hr class="session-summary-divider"\/><section class="session-summary-items session-summary-resources" aria-label="Browser contexts"/u);
    expect(html).toMatch(/aria-label="Browser contexts"[\s\S]*?<hr class="session-summary-divider"\/><section class="session-summary-items session-summary-resources" aria-label="Session resources"/u);
    expect(html.indexOf('aria-label="Open Browser Default"')).toBeLessThan(html.indexOf('aria-label="Loading session memories"'));
    expect(html).not.toContain('0 Memories');
    expect(html).not.toContain('0 Runbooks');
    expect(html).not.toContain('0 Notes');
    expect(html).not.toContain('0 Guides');
  });

  it('shows every memory type count directly in session and workspace summaries', () => {
    const baseProfile = customProfile();
    const profile = {
      ...baseProfile,
      memory: {
        ...baseProfile.memory,
        types: [...baseProfile.memory.types, {
          ...baseProfile.memory.types[0]!, id: 'reference', name: 'Reference', pluralName: 'References', order: 30
        }]
      }
    };
    const memory = {
      nodes: [
        node('memory-note', 'note', 'published'),
        node('memory-archive', 'retired_note', 'draft'),
        node('memory-rejected', 'note', 'rejected')
      ],
      runbooks: [], edges: [], contextWorkspaceId: 'workspace_one', contextSubjectId: 'subject_one'
    } as never;
    const detail = {
      run: { id: 'run_one', status: 'completed', createdAt: '2026-08-10T00:00:00.000Z' },
      attempts: [], traceEvents: [], artifacts: [], verifierContracts: [], verifierRuns: [],
      modelSessions: [], policyEvents: [], exports: []
    } as unknown as RunDetail;

    for (const viewSpace of ['session', 'workspace'] as const) {
      const html = renderToStaticMarkup(createElement(ResearchSidePanel, {
        detail, events: [], memory, researchProfile: profile, providerModelCatalog: [],
        runId: 'run_one', runStatus: null, selectedRunbook: null,
        selectedRunbookDocument: null, runbookLoading: false, runbookError: null,
        selectedSubagentPath: null, selectedRunbookId: null, searchHighlightQuery: '',
        onOpenRunbook: () => undefined, onSelectSubagent: () => undefined,
        onBackToRunbooks: () => undefined, onBackToSubagents: () => undefined,
        expanded: false, viewSpace, browserContextsEnabled: true
      }));

      expect(html).toContain('aria-label="Open Browser Default"');
      expect(html).toContain('aria-label="Browser contexts"');
      const browserSection = html.indexOf('aria-label="Browser contexts"');
      const resourceSection = html.indexOf(`aria-label="${viewSpace === 'session' ? 'Session' : 'Workspace'} resources"`);
      expect(browserSection).toBeLessThan(resourceSection);
      expect(html.slice(browserSection, resourceSection)).toContain('session-summary-divider');
      expect(html).toContain('<span>2 Memories</span>');
      expect(html.indexOf('aria-label="Open Browser Default"')).toBeLessThan(html.indexOf('<span>2 Memories</span>'));
      if (viewSpace === 'session') {
        expect(html).toContain('class="session-memory-type-overflow" aria-hidden="true" inert=""');
        expect(html).toContain('class="session-memory-type-toggle" aria-expanded="false"');
        expect(html).toContain('>Show more</button>');
      } else {
        expect(html).toContain('<div class="session-memory-type-item"><span>1 Note</span>');
        expect(html).toContain('<div class="session-memory-type-item"><span>1 Archived Note</span>');
        expect(html).toContain('<div class="session-memory-type-item"><span>0 References</span>');
        expect(html).not.toContain('session-memory-type-toggle');
        expect(html).not.toContain('session-memory-type-overflow');
      }
      expect(html).not.toContain('Boring');
    }
  });

  it('shows Leads in both summaries and restores the Campaign Leads section', () => {
    const lead = claim('lead_example', 'Example parser lead');
    lead.originSessionId = 'run_one';
    const rejectedLead = { ...claim('lead_rejected', 'Rejected example lead'), status: 'rejected' };
    const memory = { nodes: [], edges: [], runbooks: [], leads: [lead, rejectedLead], findings: [],
      contextWorkspaceId: lead.workspaceId, contextSubjectId: lead.subjectId } as never;
    const detail = { run: { id: 'run_one', status: 'completed', createdAt: '2026-08-10T00:00:00.000Z' },
      attempts: [], traceEvents: [], transcriptMessages: [], artifacts: [], verifierContracts: [], verifierRuns: [],
      modelSessions: [], policyEvents: [], exports: [] } as unknown as RunDetail;
    const sidePanelProps = {
      detail, events: [], memory, researchProfile: customProfile(), providerModelCatalog: [],
      runId: 'run_one', runStatus: null, selectedRunbook: null,
      selectedRunbookDocument: null, runbookLoading: false, runbookError: null,
      selectedSubagentPath: null, selectedRunbookId: null, searchHighlightQuery: '',
      onOpenRunbook: () => undefined, onSelectSubagent: () => undefined,
      onBackToRunbooks: () => undefined, onBackToSubagents: () => undefined,
      browserContextsEnabled: false
    };

    for (const viewSpace of ['session', 'workspace'] as const) {
      const html = renderToStaticMarkup(createElement(ResearchSidePanel, { ...sidePanelProps, viewSpace }));
      expect(html).toContain(viewSpace === 'session' ? '<span>1 Lead</span>' : '<span>2 Leads</span>');
      expect(html).toContain('lucide-lightbulb');
    }

    expect(campaignCatalogSectionVisibility('leads', 'all')).toEqual({ memories: true, leads: true, findings: false });
    expect(campaignEntryFilterSelection('leads')).toEqual({ claimFilter: 'leads', memoryType: NO_CAMPAIGN_MEMORIES_FILTER, runbookFilter: 'none' });
    expect(campaignEntryScrollTargetSelector('leads')).toBe('[data-campaign-projection="leads"]');
    const catalog = renderToStaticMarkup(createElement(CampaignClaimCatalogLists, {
      leads: [lead], findings: [], selectedClaimId: null, onOpen: () => undefined
    }));
    expect(catalog).toContain('data-campaign-projection="leads"');
    expect(catalog).toContain('Example parser lead');
    expect(catalog).not.toContain('data-campaign-projection="findings"');
    const leadDetail = renderToStaticMarkup(createElement(ResearchSidePanel, {
      ...sidePanelProps, expanded: true, selectedClaimId: lead.id
    }));
    expect(leadDetail).toContain('aria-label="Lead detail views"');
    expect(leadDetail).toContain('Example parser lead');

    const finding = claim('finding_example', 'Example parser finding');
    finding.projection = 'finding';
    const workspaceFindingDetail = renderToStaticMarkup(createElement(ResearchSidePanel, {
      ...sidePanelProps,
      memory: { nodes: [], edges: [], runbooks: [], leads: [], findings: [finding],
        contextWorkspaceId: finding.workspaceId, contextSubjectId: finding.subjectId } as never,
      viewSpace: 'workspace', expanded: true, selectedClaimId: finding.id
    }));
    expect(workspaceFindingDetail).toContain('aria-label="Open workspace detail views"');
    expect(workspaceFindingDetail).toContain('<span>Campaign</span></button>');
    expect(workspaceFindingDetail).toContain('aria-label="Back to Campaign"');
  });

  it('starts finding details on Breakdown with tabs below the status and timestamp', () => {
    const parent = claim('claim_parent', 'Canonical parser boundary');
    parent.projection = 'finding';
    parent.originSessionId = 'session_example';
    parent.stepsToReproduce = '1. Run the example parser.\n2. Observe the retained state.';
    parent.duplicateClaims = [{
      id: 'claim_duplicate',
      projection: 'lead',
      maturity: 'observed',
      rating: 'high',
      classification: 'security.vulnerability',
      title: 'Redundant parser boundary',
      status: 'observed',
      revision: 4,
      markedAt: '2026-09-02T12:00:00.000Z'
    }];
    const other = claim('claim_other', 'Canonical allocator boundary');
    const html = renderToStaticMarkup(createElement(CampaignClaimDetailView, {
      claim: parent,
      claimCandidates: [parent, other],
      mutationBusy: false,
      onMarkDuplicate: () => undefined,
      onUndoDuplicate: () => undefined
    }));

    expect(html).toContain('role="tablist" aria-label="Finding detail views"');
    expect(html.match(/role="tab"/gu)).toHaveLength(4);
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain('lucide-list-checks');
    expect(html).toContain('lucide-history');
    expect(html).toContain('lucide-link2');
    expect(html).toContain('lucide-copy');
    expect(html).toContain('<span>Breakdown</span></button>');
    expect(html).toContain('<span>History</span></button>');
    expect(html).toContain('<span>References</span></button>');
    expect(html).toContain('<span>Duplicates</span></button>');
    expect(html.indexOf('<header class="memory-detail-heading">')).toBeLessThan(html.indexOf('aria-label="Finding detail views"'));
    expect(html).toContain('role="tabpanel"');
    expect(html).not.toContain('aria-label="Duplicate finding management"');
    expect(html).not.toContain('Redundant parser boundary');
    expect(html).toContain('<span class="campaign-claim-identifier-tag">claim_parent</span><span class="campaign-claim-identifier-tag">workspace_one</span><span class="campaign-claim-identifier-tag">subject_one</span><span class="campaign-claim-identifier-tag">session_example</span>');
    expect(html).not.toContain('>Identifiers</h4>');
    expect(html).toMatch(/<strong>Steps to Reproduce<\/strong><div class="main-trace-markdown"><ol>\s*<li>Run the example parser.<\/li>\s*<li>Observe the retained state.<\/li>\s*<\/ol><\/div>/u);
    expect(html.indexOf('class="campaign-claim-reproduction-steps"')).toBeLessThan(html.indexOf('class="memory-catalog-meta"'));
    expect(html).not.toContain('<dt><strong>Freshness</strong></dt>');
    expect(html).not.toContain('<dt><strong>Workflow</strong></dt>');
    expect(html).not.toContain('<dt><strong>Finding</strong></dt>');
  });

  it('renders finding breakdown prose as Markdown without raw HTML', () => {
    const finding = claim('claim_example', 'Example parser boundary');
    finding.summary = '**Observed** parser state. <script>unexpected()</script>';
    finding.impact = '- Shared state can change.\n- A later request can read it.';
    finding.stepsToReproduce = '```sh\nexample-parser --check\n```';
    finding.staleReason = '*Recheck* the example input.';
    finding.securityTracking = {
      reachability: {
        state: 'conditional', conditions: 'Requires `example-mode`.', evidenceIds: [], assessorId: null,
        assessedAt: null, sourceRevision: null, environmentFingerprint: null
      },
      cvssAssessments: [], affectedAssetIds: [], surfaceIds: [], progressBlocker: null,
      externalReferences: []
    };
    const html = renderToStaticMarkup(createElement(CampaignClaimDetailView, { claim: finding }));

    expect(html).toContain('<strong>Observed</strong> parser state.');
    expect(html).not.toContain('<script>');
    expect(html).toContain('<li>Shared state can change.</li>');
    expect(html).toContain('<li>A later request can read it.</li>');
    expect(html).toContain('<code class="main-trace-inline-code">example-mode</code>');
    expect(html).toContain('main-trace-markdown-code-block');
    expect(html).toContain('example-parser --check');
    expect(html).toContain('<em>Recheck</em> the example input.');
  });

  it('shows a blocked finding notice immediately below the view tabs', () => {
    const finding = claim('claim_example', 'Example parser boundary');
    finding.securityTracking = {
      reachability: {
        state: 'not_assessed', conditions: '', evidenceIds: [], assessorId: null,
        assessedAt: null, sourceRevision: null, environmentFingerprint: null
      },
      cvssAssessments: [], affectedAssetIds: [], surfaceIds: [],
      progressBlocker: { reason: 'Authorized routes were tested and cannot reach this primitive; a new entry path is needed.', setAt: '2026-09-02T12:00:00.000Z' },
      externalReferences: []
    };
    const html = renderToStaticMarkup(createElement(CampaignClaimDetailView, { claim: finding }));

    expect(html).toContain('<section class="campaign-claim-blocker" aria-label="Blocked"><h3>');
    expect(html).toContain('lucide-octagon-minus');
    expect(html).toContain('Blocked</h3><p>Authorized routes were tested and cannot reach this primitive; a new entry path is needed.</p></section>');
    expect(html.indexOf('aria-label="Finding detail views"')).toBeLessThan(html.indexOf('class="campaign-claim-blocker"'));
    expect(html.indexOf('class="campaign-claim-blocker"')).toBeLessThan(html.indexOf('role="tabpanel"'));
    expect(html).not.toContain('Progress blocker:');
  });

  it('shows finding status and model authors beside the detail timestamp without repeating the title', () => {
    const finding = claim('claim_example', 'Example parser boundary');
    finding.projection = 'finding';
    finding.authors = [{ provider: 'openai', model: 'gpt-5.6' }];
    const html = renderToStaticMarkup(createElement(CampaignClaimDetailView, {
      claim: finding
    }));
    const header = html.match(/<header class="memory-detail-heading">([\s\S]*?)<\/header>/)?.[1] ?? '';

    expect(header).toContain('campaign-claim-detail-attribution');
    expect(header).toContain('>Observed by</span>');
    expect(header).toContain('aria-label="Model authors"');
    expect(header).toContain('memory-catalog-item-author-provider');
    expect(header).toContain('gpt-5.6');
    expect(header).toContain(`<time dateTime="${finding.updatedAt}" title="${formatSessionDateTime(finding.updatedAt)}">${formatSessionDateTime(finding.updatedAt)}</time>`);
    expect(header).not.toContain('catalog-time-since');
    expect(header).not.toContain('Example parser boundary');
    expect(header).not.toContain('<h3>');
    expect(header.indexOf('aria-label="Model authors"')).toBeLessThan(header.indexOf('<time'));
  });

  it('uses the rating and assessed reachability as breakdown headings without redundant rows', () => {
    const finding = claim('claim_example', 'Example parser boundary');
    finding.impact = 'An example request changes shared parser state.';
    const render = () => renderToStaticMarkup(createElement(CampaignClaimDetailView, { claim: finding }));
    const unassessed = render();
    expect(unassessed).toContain('<strong>High Impact</strong>');
    expect(unassessed).toContain('<strong>Reachability</strong>');
    for (const label of ['Untrusted rating', 'Maturity', 'Reachability']) {
      expect(unassessed).not.toContain(`<dt><strong>${label}</strong></dt>`);
    }

    for (const [state, heading] of [
      ['reachable', 'Reachable'],
      ['unreachable', 'Unreachable'],
      ['conditional', 'Conditionally Reachable']
    ] as const) {
      finding.securityTracking = {
        reachability: {
          state, conditions: 'Example boundary condition.', evidenceIds: [], assessorId: null,
          assessedAt: null, sourceRevision: null, environmentFingerprint: null
        },
        cvssAssessments: [], affectedAssetIds: [], surfaceIds: [], progressBlocker: null,
        externalReferences: []
      };
      const html = render();
      expect(html).toContain(`<strong>${heading}</strong><div class="main-trace-markdown"><p>Example boundary condition.</p></div>`);
      expect(html).not.toContain('<dt><strong>Reachability</strong></dt>');
    }
  });

  it('shows an unrated CVSS meter and fills fractional segments for the latest assessment', () => {
    const finding = claim('claim_example', 'Example parser boundary');
    const render = () => renderToStaticMarkup(createElement(CampaignClaimDetailView, { claim: finding }));
    const unrated = render();
    expect(unrated).toContain('class="campaign-claim-cvss-meter" title="CVSS 3.1"><span>Unrated</span>');
    expect(unrated).toContain('<strong>Steps to Reproduce</strong><em>Not recorded</em>');
    expect(unrated.match(/class="campaign-claim-cvss-segment"/gu)).toHaveLength(10);
    expect(unrated.match(/style="width:0%"/gu)).toHaveLength(10);

    finding.securityTracking = {
      reachability: {
        state: 'not_assessed', conditions: '', evidenceIds: [], assessorId: null,
        assessedAt: null, sourceRevision: null, environmentFingerprint: null
      },
      cvssAssessments: [{
        version: '4.0', vector: 'CVSS:4.0/AV:N', score: 7.4, nomenclature: 'CVSS-B',
        assessorId: 'reviewer-example', assessedAt: '2026-09-02T12:00:00.000Z', environmentFingerprint: null
      }],
      affectedAssetIds: ['asset_example_parser'], surfaceIds: ['surface_example_parser'], progressBlocker: null,
      externalReferences: []
    };
    const rated = render();
    expect(rated).toContain('class="campaign-claim-cvss-meter" title="CVSS 4.0"><span>7.4</span>');
    expect(rated).not.toContain('<dt><strong>CVSS</strong></dt>');
    expect(rated).toContain('<span class="campaign-claim-identifier-tag">claim_example</span><span class="campaign-claim-identifier-tag">workspace_one</span><span class="campaign-claim-identifier-tag">subject_one</span><span class="campaign-claim-identifier-tag">No origin session</span><span class="campaign-claim-identifier-tag">asset_example_parser</span><span class="campaign-claim-identifier-tag">surface_example_parser</span>');
    expect(rated).not.toContain('>Surfaces</span>');
    expect(rated).not.toContain('Affected assets');
    expect(rated).not.toContain('Affected Versions');
    expect(rated).not.toContain('CVSS Assessments');
    expect(rated.indexOf('class="memory-catalog-meta"')).toBeGreaterThan(rated.indexOf('class="campaign-claim-reproduction-steps"'));
    expect(rated.match(/class="campaign-claim-cvss-segment"/gu)).toHaveLength(10);
    expect(rated.match(/style="width:100%"/gu)).toHaveLength(7);
    expect(rated.match(/style="width:40%"/gu)).toHaveLength(1);
    expect(rated.match(/style="width:0%"/gu)).toHaveLength(2);
  });

  it('renders product-wide attention colors in Appearance with a light and dark toggle', () => {
    const html = renderToStaticMarkup(createElement(AppearanceSettingsView, {
      theme: 'dark',
      onChangeTheme: () => undefined,
      sessionHeatPreferences: {
        heatOverrides: {},
        paletteOverrides: {
          attention: {
            light: { low: '#112233' },
            dark: { critical: '#445566' }
          }
        }
      },
      onSetSessionHeatPalettePreference: () => undefined
    }));

    expect(html).toContain('<h2 id="profile-heat-heading">Heat Palette</h2>');
    expect(html).toContain('role="group" aria-label="Heat variant"');
    expect(html).toContain('aria-pressed="false">Light</button>');
    expect(html).toContain('class="active" type="button" aria-pressed="true">Dark</button>');
    expect(html).not.toContain('aria-label="Reset Light Heat colors"');
    expect(html).toContain('aria-label="Reset Dark Heat colors"');
    expect(html.indexOf('aria-label="Reset Dark Heat colors"')).toBeLessThan(
      html.indexOf('role="group" aria-label="Heat variant"')
    );
    expect(html).not.toContain('aria-label="Light Heat Low session heat color"');
    expect(html).not.toContain('value="#112233"');
    expect(html).toContain('aria-label="Dark Heat Critical session heat color"');
    expect(html).toContain('value="#445566"');
    expect(html).toContain('A subtle signal for sessions with light activity.');
    expect(html).toContain('The strongest signal for sessions with exceptional activity.');
    expect(html).toContain('aria-label="Research attention colors"');
  });
});

function claim(id: string, title: string): AppServerFindingSummary {
  return {
    id,
    workspaceId: 'workspace_one',
    subjectId: 'subject_one',
    memoryNodeId: null,
    originSessionId: null,
    projection: 'lead',
    maturity: 'observed',
    rating: 'high',
    classification: 'security.vulnerability',

    duplicateOfClaimId: null,
    duplicateMarkedAt: null,
    duplicateClaims: [],
    title,
    summary: 'A directly observed parser boundary.',
    impact: '',
    stepsToReproduce: '',
    securityTracking: null,
    status: 'observed',
    operatorOverride: false,
    staleFromStatus: null,
    confidence: 0.8,
    sourceRevision: null,
    environmentFingerprint: null,
    reproductionRunbookId: null,
    reportId: null,
    disclosureReference: null,
    staleReason: null,
    evidence: [],
    transitions: [],
    authors: [],
    createdAt: '2026-09-02T12:00:00.000Z',
    updatedAt: '2026-09-02T12:00:00.000Z',
    revision: 2
  };
}

function customProfile(): ResearchProfile {
  const base = testResearchProfile();
  return {
    ...base,
    memory: {
      ...base.memory,
      types: [
        {
          id: 'note',
          name: 'Note',
          pluralName: 'Notes',
          description: 'A durable note.',
          lifecycle: 'active',
          creatable: true,
          group: 'Knowledge',
          color: '#123456',
          order: 20,
          defaultStatus: 'draft',
          allowedStatuses: ['draft', 'published'],
          sessionHeat: { published: 'high' }
        },
        {
          id: 'retired_note',
          name: 'Archived Note',
          pluralName: 'Archived Notes',
          description: 'A historical note type.',
          lifecycle: 'retired',
          creatable: false,
          group: 'Archive',
          color: '#778899',
          order: 10,
          defaultStatus: 'draft',
          allowedStatuses: ['draft']
        }
      ],
      statuses: [
        { id: 'published', name: 'Published', description: 'Ready to share.', order: 20, terminal: true, polarity: 'positive' },
        { id: 'draft', name: 'Working', description: 'Still developing.', order: 10, polarity: 'neutral' }
      ]
    },
    workflows: [workflow('survey', 'Survey', 2, true), workflow('synthesize', 'Synthesize', 3)],
    presentation: {
      newResearchLabel: 'New Inquiry',
      memoryLabel: 'Note',
      runbookLabel: 'Guides',
      sessionLabel: 'Study'
    }
  };
}

function workflow(id: string, name: string, goalSuggestionCount: number, isDefault = false): ResearchProfileWorkflow {
  return {
    id,
    name,
    description: `${name} the available material.`,
    goalSuggestionCount,
    goalSuggestionInstructions: [],
    promptInstructions: [],
    outputRequirements: [],
    ...(isDefault ? { default: true } : {})
  };
}

function snapshot(profile: ResearchProfile): WorkspaceSnapshot {
  return {
    workspace: { workspaceId: 'workspace_one' },
    activeScope: { id: 'scope_one' },
    researchProfile: {
      id: 'snapshot_one',
      workspaceId: 'workspace_one',
      profileId: profile.id,
      profileVersion: profile.version,
      profileHash: 'a'.repeat(64),
      source: 'explicit',
      sourcePath: '.beale/profile.json',
      profile,
      active: true,
      createdAt: '2026-08-10T00:00:00.000Z'
    }
  } as WorkspaceSnapshot;
}

function node(id: string, type: string, status: string): AppServerMemoryNodeSummary {
  return {
    id,
    sessionIds: ['run_one'],
    workspaces: [{ id: 'workspace_one', name: 'Workspace' }],
    subjectId: 'subject_one',
    subjectName: 'Subject',
    type,
    title: id,
    summary: '',
    body: '',
    status,
    confidence: 0.5,
    assetIds: [],
    tags: [],
    attributes: {},
    evidenceRefs: [],
    createdAt: '2026-08-10T00:00:00.000Z',
    updatedAt: '2026-08-10T00:00:00.000Z',
    revision: 1
  };
}
