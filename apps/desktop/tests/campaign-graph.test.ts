import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { layoutCampaignGraph } from '../src/renderer/view-models/campaignGraph';
import { campaignClaimRatingPresentation } from '../src/renderer/view-models/campaignClaims';
import { CampaignBoardView, adjacentCampaignBoardMaturity, campaignBoardAuthorOptions, campaignBoardClaimMetadata, campaignBoardFindings } from '../src/renderer/features/workspaces/CampaignGraphView';
import { CampaignClaimCatalogSection } from '../src/renderer/features/research/MemorySidePanel';
import { findingRevisionContext } from '../src/main/findingRevisionContext';
import type { AppServerFindingSummary, AppServerMemorySummary, WorkspaceScopeVersion } from '@shared/types';

describe('campaign graph projection', () => {
  it('lays out assets, memory, findings, and proof deterministically while dropping dangling edges', () => {
    const nodes = [
      { id: 'finding:f1', kind: 'finding', label: 'Finding', status: 'observed', memoryNodeId: 'm1', findingId: 'f1', assetId: null, evidenceCount: 1, updatedAt: '2026-01-01T00:00:00Z' },
      { id: 'asset:a1', kind: 'asset', label: 'Asset', status: 'covered', memoryNodeId: null, findingId: null, assetId: 'a1', evidenceCount: 0, updatedAt: '2026-01-01T00:00:00Z' },
      { id: 'memory:m1', kind: 'memory', label: 'Memory', status: 'suspected', memoryNodeId: 'm1', findingId: null, assetId: null, evidenceCount: 1, updatedAt: '2026-01-01T00:00:00Z' },
      { id: 'runbook:r1', kind: 'runbook', label: 'Proof', status: 'active', memoryNodeId: null, findingId: null, assetId: null, evidenceCount: 1, updatedAt: '2026-01-01T00:00:00Z' }
    ] as const;
    const layout = layoutCampaignGraph(nodes, [
      { fromId: 'asset:a1', toId: 'memory:m1', relation: 'covered_by', contradictory: false },
      { fromId: 'finding:f1', toId: 'runbook:r1', relation: 'reproduced_by', contradictory: false },
      { fromId: 'missing', toId: 'memory:m1', relation: 'dangling', contradictory: false }
    ]);

    expect(layout.nodes.map(({ id, x }) => [id, x])).toEqual([
      ['asset:a1', 16], ['memory:m1', 236], ['finding:f1', 456], ['runbook:r1', 676]
    ]);
    expect(layout.edges).toHaveLength(2);
    expect(layout.width).toBe(876);
  });

  it('changes finding revision identity when source or execution environment changes', () => {
    const scope = scopeFixture();
    const original = findingRevisionContext(scope);
    const sourceChanged = findingRevisionContext({
      ...scope,
      assets: [{ ...scope.assets[0]!, attributes: { ...scope.assets[0]!.attributes, head: 'commit-two' } }]
    });
    const environmentChanged = findingRevisionContext({
      ...scope,
      assets: [{ ...scope.assets[0]!, attributes: { ...scope.assets[0]!.attributes, targetVersion: '2.0' } }]
    });
    expect(original.assetIds).toEqual(['asset_repo']);
    expect(sourceChanged.sourceRevision).not.toBe(original.sourceRevision);
    expect(sourceChanged.environmentFingerprint).toBe(original.environmentFingerprint);
    expect(environmentChanged.environmentFingerprint).not.toBe(original.environmentFingerprint);
  });

  it('shows the preferred rating on Board cards', () => {
    const claim = {
      projection: 'finding',
      maturity: 'reproduced',
      rating: 'low',
      classification: 'security.vulnerability',
      securityTracking: {
        cvssAssessments: [{ score: 8.2 }]
      }
    } as AppServerFindingSummary;

    expect(campaignClaimRatingPresentation(claim).label).toBe('High (CVSS 8.2)');
    expect(campaignBoardClaimMetadata(claim)).toBe('High (CVSS 8.2)');
    expect(campaignBoardClaimMetadata(claim)).not.toContain('Primitive');
    expect(campaignBoardFindings({ findings: [claim] } as AppServerMemorySummary, 'reproduced', {
      rating: 'high'
    })).toEqual([claim]);
  });

  it('falls back to the untrusted rating when a Board claim has no CVSS assessment', () => {
    const claim = {
      rating: 'medium',
      classification: 'security.vulnerability',
      securityTracking: null
    } as AppServerFindingSummary;

    expect(campaignBoardClaimMetadata(claim)).toBe('Medium');
    expect(campaignBoardClaimMetadata(claim)).not.toContain('Chain');
  });

  it('places author filtering beside rating beneath the Findings title', () => {
    const html = renderToStaticMarkup(createElement(CampaignBoardView, {
      memory: null,
      providerModelCatalog: [],
      workspaceName: 'Example Workspace',
      onOpenClaim: () => undefined
    }));

    expect(html).toContain('Example Workspace Findings');
    expect(html).toMatch(/Example Workspace Findings<\/h2><div class="campaign-board-title-search" role="search"><button aria-label="Search findings"/u);
    expect(html).toMatch(/<\/button><\/div><\/div><div class="campaign-board-controls-row"><div class="campaign-board-filters"/u);
    expect(html).not.toContain('Findings are grouped by maturity.');
    expect(html).not.toContain('aria-label="Finding class filter"');
    expect(html).not.toContain('aria-label="Filter findings"');
    expect(html.indexOf('aria-label="Finding rating filter"')).toBeLessThan(html.indexOf('aria-label="Finding author filter"'));
    expect(html).toContain('All Authors');
    expect(html.indexOf('aria-label="Finding rating filter"')).toBeLessThan(html.indexOf('0 refuted hidden'));
    expect(html).toContain('campaign-board-lead-heading');
    expect(html).not.toContain('campaign-board-reported-heading');
    expect(html).not.toContain('campaign-board-refuted-heading');
    expect(html.match(/class="main-side-scroll campaign-board-lane-scroll"/gu)).toHaveLength(4);
    expect(html.match(/class="campaign-board-lane-list"/gu)).toHaveLength(4);
    expect(html.match(/class="workspace-campaign-list-heading campaign-board-lane-heading"/gu)).toHaveLength(4);
  });

  it('shows one focused lane with adjacent navigation when the right area is expanded', () => {
    const props = { memory: null, providerModelCatalog: [], workspaceName: 'Example Workspace', onOpenClaim: () => undefined };
    const compact = renderToStaticMarkup(createElement(CampaignBoardView, { ...props, compact: true }));
    const verified = renderToStaticMarkup(createElement(CampaignBoardView, { ...props, compact: true, focusedMaturity: 'verified' }));

    expect(compact).toContain('campaign-board-lanes is-compact');
    expect(compact).toContain('campaign-board-panel is-compact');
    expect(compact).toContain('campaign-board-controls-row');
    expect(compact).not.toContain('Use the arrows to switch findings lanes.');
    expect(compact).not.toContain('aria-label="Filter findings"');
    expect(compact).not.toContain('aria-label="Search findings"');
    expect(compact).not.toContain('refuted hidden');
    expect(compact).not.toContain('aria-label="Finding class filter"');
    expect(compact).toContain('aria-label="Finding rating filter"');
    expect(compact).toContain('aria-label="Finding author filter"');
    expect(compact).toContain('campaign-board-lead-heading');
    expect(compact).not.toContain('campaign-board-reproduced-heading');
    expect(compact.match(/class="workspace-campaign-list-heading campaign-board-lane-heading"/gu)).toHaveLength(1);
    expect(compact).toContain('aria-label="Previous findings lane"');
    expect(compact).toContain('aria-label="Next findings lane"');
    expect(verified).toContain('campaign-board-verified-heading');
    expect(verified).not.toContain('campaign-board-lead-heading');
    expect(adjacentCampaignBoardMaturity('lead', -1)).toBeNull();
    expect(adjacentCampaignBoardMaturity('lead', 1)).toBe('observed');
    expect(adjacentCampaignBoardMaturity('observed', -1)).toBe('lead');
    expect(adjacentCampaignBoardMaturity('observed', 1)).toBe('reproduced');
    expect(adjacentCampaignBoardMaturity('verified', 1)).toBeNull();
  });

  it('shows Leads in the first lane and includes them in board filters', () => {
    const lead = {
      id: 'lead-example-parser', projection: 'lead', maturity: 'lead',
      title: 'Example parser lead', summary: 'Inspect the example branch.', rating: 'medium',
      classification: 'security.vulnerability', impact: '', status: 'hypothesis',
      authors: [{ provider: 'openai-codex', model: 'example-model' }], evidence: [],
      securityTracking: null, revision: 1
    } as unknown as AppServerFindingSummary;
    const memory = { leads: [lead], findings: [] } as unknown as AppServerMemorySummary;
    expect(campaignBoardFindings(memory, 'lead')).toEqual([lead]);
    expect(campaignBoardFindings(memory, 'observed')).toEqual([]);
    expect(campaignBoardAuthorOptions(memory, [])).toHaveLength(2);
    const html = renderToStaticMarkup(createElement(CampaignBoardView, {
      memory, providerModelCatalog: [], workspaceName: 'Example Workspace', onOpenClaim: () => undefined,
      onTransitionClaim: async () => undefined
    }));
    expect(html).toContain('campaign-board-lead-heading');
    expect(html).toContain('Example parser lead');
    expect(html).toContain('draggable="true"');
  });

  it('makes finding cards draggable when a canonical board transition is available', () => {
    const finding = {
      id: 'finding-example-parser', projection: 'finding', maturity: 'observed',
      title: 'Example parser boundary', rating: 'medium', classification: 'security.vulnerability',
      authors: [], evidence: [], securityTracking: null, revision: 2
    } as unknown as AppServerFindingSummary;
    const props = {
      memory: { findings: [finding] } as AppServerMemorySummary,
      providerModelCatalog: [], workspaceName: 'Example Workspace',
      onOpenClaim: () => undefined
    };
    const readOnly = renderToStaticMarkup(createElement(CampaignBoardView, props));
    const editable = renderToStaticMarkup(createElement(CampaignBoardView, {
      ...props,
      onTransitionClaim: async () => undefined
    }));
    expect(readOnly).toContain('draggable="false"');
    expect(editable).toContain('draggable="true"');
    expect(editable).not.toContain('Drag findings between columns to change status.');
  });

  it('marks only the finding open in the detail panel as active', () => {
    const finding = {
      id: 'finding-example-parser', projection: 'finding', maturity: 'observed',
      title: 'Example parser boundary', rating: 'medium', classification: 'security.vulnerability',
      authors: [], evidence: [], securityTracking: null, revision: 1
    } as unknown as AppServerFindingSummary;
    const props = {
      memory: { findings: [finding, { ...finding, id: 'finding-example-chain', title: 'Example chain' }] } as AppServerMemorySummary,
      providerModelCatalog: [], workspaceName: 'Example Workspace', onOpenClaim: () => undefined
    };
    const open = renderToStaticMarkup(createElement(CampaignBoardView, { ...props, openClaimId: finding.id }));
    const closed = renderToStaticMarkup(createElement(CampaignBoardView, { ...props, openClaimId: null }));

    expect(open.match(/class="campaign-claim-card campaign-board-card is-active"/gu)).toHaveLength(1);
    expect(open).toMatch(/aria-current="true" class="campaign-claim-card campaign-board-card is-active"[^>]*>.*?Example parser boundary/su);
    expect(closed).not.toContain('is-active');
    expect(closed).not.toContain('aria-current="true"');
  });

  it('counts refuted findings outside the board lanes', () => {
    const refuted = {
      id: 'finding-example-refuted', projection: 'finding', maturity: 'refuted',
      title: 'Example disproved path', rating: 'low', classification: 'security.vulnerability',
      authors: [], evidence: [], securityTracking: null, revision: 2
    } as unknown as AppServerFindingSummary;
    const refutedLead = { ...refuted, id: 'lead-example-refuted', projection: 'lead', title: 'Example rejected lead' } as AppServerFindingSummary;
    const html = renderToStaticMarkup(createElement(CampaignBoardView, {
      memory: { leads: [refutedLead], findings: [refuted] } as unknown as AppServerMemorySummary,
      providerModelCatalog: [], workspaceName: 'Example Workspace', onOpenClaim: () => undefined
    }));
    expect(html).toContain('2 refuted hidden');
    expect(html).not.toContain('Example disproved path');
    expect(html).not.toContain('campaign-board-refuted-heading');
  });

  it('hides refuted findings in the expanded catalog and puts their count on the right', () => {
    const observed = {
      id: 'finding-example-observed', projection: 'finding', maturity: 'observed',
      title: 'Example observed path', rating: 'low', classification: 'security.vulnerability',
      authors: [], evidence: [], securityTracking: null, updatedAt: '2026-01-01T00:00:00Z', revision: 1
    } as unknown as AppServerFindingSummary;
    const refuted = { ...observed, id: 'finding-example-refuted', maturity: 'refuted', title: 'Example disproved path' } as AppServerFindingSummary;
    const render = (claims: AppServerFindingSummary[]) => renderToStaticMarkup(createElement(CampaignClaimCatalogSection, {
      claims, label: 'Findings', selectedClaimId: null, onOpen: () => undefined
    }));
    const html = render([observed, refuted]);

    expect(html).toContain('<h3>2 Findings</h3>');
    expect(html).toContain('1 refuted hidden');
    expect(html).not.toContain('Unrefuted');
    expect(html).toContain('Example observed path');
    expect(html).not.toContain('Example disproved path');
    expect(render([refuted])).toContain('No unrefuted findings.');
  });

  it('previews five leads or findings and offers the remaining claims on demand', () => {
    for (const label of ['Leads', 'Findings'] as const) {
      const claims = Array.from({ length: 7 }, (_, index) => ({
        id: `claim-example-${index + 1}`,
        projection: label === 'Leads' ? 'lead' : 'finding',
        maturity: 'observed',
        title: `Example claim ${index + 1}`,
        rating: 'medium',
        classification: 'security.primitive',
        authors: [], evidence: [], securityTracking: null,
        updatedAt: '2026-01-01T00:00:00Z', revision: 1
      } as unknown as AppServerFindingSummary));
      const html = renderToStaticMarkup(createElement(CampaignClaimCatalogSection, {
        claims, label, selectedClaimId: null, onOpen: () => undefined
      }));
      expect(html).toContain('Show 2 more');
      expect(html).toContain('aria-expanded="false"');
      expect(html).toContain('Example claim 5');
      expect(html).not.toContain('Example claim 6');
      expect(html).not.toContain('Example claim 7');
    }
  });

  it('shows a stop sign only for blocked board findings', () => {
    const finding = {
      id: 'finding-example-blocked', projection: 'finding', maturity: 'observed',
      title: 'Example blocked path', rating: 'medium', classification: 'security.vulnerability',
      authors: [], evidence: [], revision: 1, securityTracking: null
    } as unknown as AppServerFindingSummary;
    const render = (claim: AppServerFindingSummary) => renderToStaticMarkup(createElement(CampaignBoardView, {
      memory: { findings: [claim] } as AppServerMemorySummary,
      providerModelCatalog: [], workspaceName: 'Example Workspace', onOpenClaim: () => undefined
    }));
    const unblocked = render(finding);
    const blocked = render({
      ...finding,
      securityTracking: { cvssAssessments: [], progressBlocker: { reason: 'Authorized routes cannot reach this sink; a new path is required.', setAt: '2026-01-01T00:00:00Z' } }
    } as unknown as AppServerFindingSummary);

    expect(unblocked).not.toContain('lucide-badge-check');
    expect(unblocked).not.toContain('lucide-octagon-minus');
    expect(blocked).toContain('lucide-octagon-minus');
    expect(blocked).toContain('aria-label="Blocked"');
    expect(blocked).toContain('Blocked: Authorized routes cannot reach this sink; a new path is required.');
  });

  it('filters Board findings by text and rating', () => {
    const parserClaim = {
      id: 'finding-example-parser',
      maturity: 'reproduced',
      rating: 'high',
      classification: 'security.vulnerability',
      title: 'Example parser boundary',
      summary: 'Unexpected payload handling',
      impact: 'Synthetic impact',
      status: 'reproduced',
      evidence: [{ summary: 'Example proof note' }],
      securityTracking: null
    } as AppServerFindingSummary;
    const otherClaim = {
      ...parserClaim,
      id: 'finding-example-other',
      classification: 'security.vulnerability',
      title: 'Example separate finding',
      summary: 'Separate finding',
      evidence: []
    };
    const memory = { findings: [parserClaim, otherClaim] } as AppServerMemorySummary;

    expect(campaignBoardFindings(memory, 'reproduced', { rating: 'all', query: '  PARSER  ' })).toEqual([parserClaim]);
    expect(campaignBoardFindings(memory, 'reproduced', { rating: 'high', query: 'proof note' })).toEqual([parserClaim]);
    expect(campaignBoardFindings(memory, 'reproduced', { rating: 'all', query: 'security.vulnerability' })).toEqual([parserClaim, otherClaim]);
    expect(campaignBoardFindings(memory, 'reproduced', { rating: 'all', query: 'missing' })).toEqual([]);
    expect(campaignBoardFindings(memory, 'reproduced', { rating: 'all', query: '  ' })).toEqual([parserClaim, otherClaim]);
  });

  it('lists model authors once and filters findings by provider and model', () => {
    const firstAuthor = { provider: 'provider-one', model: 'shared-model' };
    const secondAuthor = { provider: 'provider-two', model: 'shared-model' };
    const first = { id: 'finding-one', maturity: 'observed', rating: 'medium', authors: [firstAuthor, secondAuthor] } as AppServerFindingSummary;
    const second = { id: 'finding-two', maturity: 'observed', rating: 'high', authors: [secondAuthor] } as AppServerFindingSummary;
    const refuted = { id: 'finding-refuted', maturity: 'refuted', rating: 'low', authors: [{ provider: 'provider-three', model: 'refuted-model' }] } as AppServerFindingSummary;
    const memory = { findings: [first, second, refuted] } as AppServerMemorySummary;
    const options = campaignBoardAuthorOptions(memory, []);

    expect(options).toEqual([
      { value: 'all', label: 'All Authors' },
      { value: JSON.stringify(['provider-one', 'shared-model']), label: 'shared-model (provider-one)' },
      { value: JSON.stringify(['provider-two', 'shared-model']), label: 'shared-model (provider-two)' }
    ]);
    expect(campaignBoardFindings(memory, 'observed', { rating: 'all', author: options[1]!.value })).toEqual([first]);
    expect(campaignBoardFindings(memory, 'observed', { rating: 'high', author: options[2]!.value })).toEqual([second]);
    expect(campaignBoardFindings(memory, 'observed', { rating: 'all' })).toEqual([first, second]);
  });
});

function scopeFixture(): WorkspaceScopeVersion {
  return {
    id: 'scope_one', version: 1, status: 'active', workspaceName: 'Workspace', scopeOwner: 'Owner',
    descriptionMarkdown: '', rulesMarkdown: '', activeFrom: '2026-01-01', expiresAt: null,
    createdAt: '2026-01-01', createdBy: 'test',
    assets: [{ id: 'asset_repo', scopeVersionId: 'scope_one', createdAt: '2026-01-01', direction: 'in_scope', kind: 'repo', value: 'repo', sensitivity: 'internal', attributes: { head: 'commit-one', targetVersion: '1.0' } }]
  };
}
