import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, test } from 'vitest';
import type { BealeWorkspaceSurfaceMap } from '@beale/app-server-runtime/protocol';
import type { AppServerFindingSummary, AppServerMemoryNodeSummary } from '../src/shared/types';
import { WorkspaceSurfaceMap } from '../src/renderer/features/workspaces/WorkspaceSurfaceMap';
import { WorkspaceSurfaceDetails } from '../src/renderer/features/workspaces/WorkspaceSurfaceDetails';
import { clampDiagramView, INITIAL_DIAGRAM_VIEW, zoomDiagramView } from '../src/renderer/features/workspaces/threatDiagramViewport';
import { filterSurfaceHierarchy, layoutSurfaceHierarchy, sessionSurfaceCounts, sessionSurfaceHierarchy, surfaceCoveragePercent } from '../src/renderer/view-models/surfaceMap';

type Surface = BealeWorkspaceSurfaceMap['surfaces'][number];
const surface = (id: string, parentId: string | null, firstTouchedAt: string | null, hasCoverage = false, complete = false): Surface => ({
  id, parentId, resourceId: 'asset-example', name: id, description: `${id} boundary`, firstTouchedAt,
  foundation: hasCoverage ? { coverage: { mappingComplete: false, sinksInvestigated: false, openQuestions: [], unexploredConnections: [] } } : null,
  progressBlocker: null,
  coverageState: { covered: complete, unmetRequirements: complete ? [] : ['threat_model'], openLeadIds: [], unblockedFindingIds: [] }
});
const surfaces: Surface[] = [
  surface('asset-example', null, null),
  surface('parser', 'asset-example', '2026-01-01T00:00:00.000Z', true),
  surface('sink', 'parser', '2026-01-01T00:01:00.000Z', true, true),
  surface('untouched-sibling', 'asset-example', null),
  surface('other-root', null, '2026-01-01T00:02:00.000Z')
];

test('surface breakdown shows a scored pentagon with accessible factor values and assessment bases', () => {
  const selected: Surface = { ...surfaces[1]!, foundation: { ...surfaces[1]!.foundation!, ranking: {
    impact: { score: 9, basis: 'Synthetic state integrity consequence.' },
    reachability: { score: 8, basis: 'Synthetic public request path.' },
    relevance: { score: 7, basis: 'Synthetic scope match.' },
    exposure: { score: 6, basis: 'Synthetic pre-auth path.' },
    precedence: { score: 4, basis: 'Synthetic related prior advisory.' }
  } } };
  const map: BealeWorkspaceSurfaceMap = { schemaVersion: 1, workspaceId: 'workspace-example',
    surfaces: surfaces.map((item) => item.id === selected.id ? selected : item), agents: [] };
  const html = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: selected, map, onClose: () => undefined
  }));
  expect(html).toContain('workspace-surface-ranking-area');
  expect(html).toContain('aria-label="Surface ranking: Impact 9 of 10, Reachability 8 of 10, Relevance 7 of 10, Exposure 6 of 10, Precedence 4 of 10"');
  expect(html).toContain('Average 6.8/10');
  expect(html).toContain('Assessment basis');
  expect(html).toContain('Synthetic related prior advisory.');
});

test('ancestors stay visible, while untouched branches stay hidden', () => {
  expect(filterSurfaceHierarchy(surfaces, '', false).map((item) => item.id))
    .toEqual(['asset-example', 'parser', 'sink', 'other-root']);
  expect(filterSurfaceHierarchy(surfaces, '', true)).toEqual(surfaces);
  expect(layoutSurfaceHierarchy(filterSurfaceHierarchy(surfaces, '', false)).nodes
    .find((node) => node.surface.id === 'sink')?.depth).toBe(2);
});

test('filter preserves the full ancestor path and descendant branch', () => {
  expect(filterSurfaceHierarchy(surfaces, 'parser', false).map((item) => item.id))
    .toEqual(['asset-example', 'parser', 'sink']);
  expect(filterSurfaceHierarchy(surfaces, 'sink', true).map((item) => item.id))
    .toEqual(['asset-example', 'parser', 'sink']);
  expect(filterSurfaceHierarchy(surfaces, 'asset-example', true).map((item) => item.id))
    .toEqual(['asset-example', 'parser', 'sink', 'untouched-sibling']);
  expect(filterSurfaceHierarchy(surfaces, 'missing', true)).toEqual([]);
  expect(layoutSurfaceHierarchy(filterSurfaceHierarchy(surfaces, 'parser', false)).nodes.find((node) => node.surface.id === 'sink')?.depth).toBe(2);
});

test('surface progress reaches 100 only after threat modeling and claim resolution', () => {
  expect(surfaceCoveragePercent(surfaces[0]!)).toBe(0);
  expect(surfaceCoveragePercent(surfaces[1]!)).toBe(25);
  const modeled: Surface = { ...surfaces[1]!,
    foundation: { ...surfaces[1]!.foundation!, threatModelReviewed: true },
    coverageState: { covered: false, unmetRequirements: ['open_leads', 'unblocked_findings'],
      openLeadIds: ['lead-example'], unblockedFindingIds: ['finding-example'] } };
  expect(surfaceCoveragePercent(modeled)).toBe(50);
  expect(surfaceCoveragePercent({ ...modeled, foundation: {
    ...modeled.foundation!, threatModelReviewed: false,
    threatModel: { securityObjectives: ['Protect example state.'], assumptions: [], risks: [] }
  } })).toBe(50);
  expect(surfaceCoveragePercent({ ...modeled, coverageState: { ...modeled.coverageState,
    unmetRequirements: ['unblocked_findings'], openLeadIds: [] } })).toBe(75);
  expect(surfaceCoveragePercent({ ...modeled, coverageState: { ...modeled.coverageState,
    covered: true, unmetRequirements: [], openLeadIds: [], unblockedFindingIds: [] } })).toBe(100);
});

test('expanded threat diagram zooms toward the pointer and keeps panning within the image', () => {
  const viewport = { width: 800, height: 600, contentWidth: 800, contentHeight: 400 };
  const zoomed = zoomDiagramView(INITIAL_DIAGRAM_VIEW, 2, 600, 300, viewport);
  expect(zoomed).toEqual({ scale: 2, x: -200, y: 0 });
  expect(clampDiagramView({ scale: 2, x: 1000, y: -1000 }, viewport))
    .toEqual({ scale: 2, x: 400, y: -100 });
  expect(zoomDiagramView(zoomed, 1, 600, 300, viewport)).toEqual(INITIAL_DIAGRAM_VIEW);
});

test('live map shows search and untouched controls without replay', () => {
  const map: BealeWorkspaceSurfaceMap = { schemaVersion: 1, workspaceId: 'workspace-example', surfaces, agents: [] };
  const html = renderToStaticMarkup(createElement(WorkspaceSurfaceMap, { map, workspaceName: 'Example Workspace' }));
  expect(html).toContain('<h2>Example Workspace Surfaces</h2>');
  expect(html).toContain('System components and their current research context.');
  expect(html).toContain('aria-label="Search surfaces"');
  expect(html).not.toContain('workspace-surface-map-search');
  expect(html).toContain('Show untouched surfaces');
  expect(html).toContain('3 of 5 touched · 1 complete · 0 blocked');
  expect(html).not.toContain('Replay');
  expect(html).toContain('title="asset-example"');
  expect(html).toContain('aria-label="sink, 100% coverage"');
  expect(html).toContain('var(--surface-coverage-fill) 100%, transparent 100%');
  expect(html).not.toContain('workspace-surface-node-progress');
  expect(html).toContain('title="untouched-sibling"');
  expect(html).toContain('title="other-root"');
  expect(html).not.toContain('workspace-surface-map-detail');
  expect(surfaceCoveragePercent(surfaces[0]!)).toBe(0);
});

test('session surface map keeps ancestors for layout while counting only touched surfaces', () => {
  const map: BealeWorkspaceSurfaceMap = { schemaVersion: 1, workspaceId: 'workspace-example', surfaces,
    agents: [{ sessionId: 'session-example', sessionTitle: 'Example session', agentId: 'root', surfaceId: 'sink' },
      { sessionId: 'session-other', sessionTitle: 'Other session', agentId: 'root', surfaceId: 'parser' }],
    sessionSurfaceIds: ['parser', 'sink', 'sink'] };
  expect(sessionSurfaceCounts(map)).toEqual({ total: 2, touched: 1, completed: 1 });
  const local = sessionSurfaceHierarchy(map, 'session-example');
  expect(local.surfaces.map((item) => item.id)).toEqual(['asset-example', 'parser', 'sink']);
  expect(local.agents.map((agent) => agent.sessionId)).toEqual(['session-example']);
  const html = renderToStaticMarkup(createElement(WorkspaceSurfaceMap, {
    map: local, workspaceName: 'Example Workspace', context: 'session'
  }));
  expect(html).toContain('aria-label="Session surfaces"');
  expect(html).toContain('1 Touched · 1 Completed');
  expect(html).not.toContain('title="untouched-sibling"');
});

test('resource type filter keeps each root with its descendants', () => {
  const map: BealeWorkspaceSurfaceMap = { schemaVersion: 1, workspaceId: 'workspace-example', agents: [], surfaces: [
    { ...surface('repo-root', null, null), resourceKind: 'repository' },
    { ...surface('repo-child', 'repo-root', null), resourceKind: 'repository' },
    { ...surface('service-root', null, null), resourceKind: 'service' }
  ] };
  const html = renderToStaticMarkup(createElement(WorkspaceSurfaceMap, {
    map, workspaceName: 'Example Workspace', resourceKind: 'repo'
  }));
  expect(html).toContain('title="repo-root"');
  expect(html).toContain('title="repo-child"');
  expect(html).not.toContain('title="service-root"');
  expect(html).toContain('0 of 2 touched');
});

test('selected surface details show blockers, foundation state, and assigned claims', () => {
  const selected: Surface = {
    ...surfaces[1]!,
    firstTouchedBy: 'root',
    firstTouchedModel: 'gpt-example',
    firstTouchedProvider: 'openai-codex',
    updatedAt: '2026-01-01T00:04:00.000Z',
    foundation: {
      discovery: 'The example service exposes a request parser behind the public API.',
      techStack: [{ technology: 'Example Parser', role: 'Parses requests', version: '2.x', environment: 'Example service',
        evidence: 'Synthetic package manifest', confidence: 'observed' }],
      assets: [{ name: 'src/request_parser.ts', status: 'untouched' },
        { name: 'parseExampleRequest()', status: 'in_progress' },
        { name: 'https://example.test/parser', status: 'complete' }],
      bugHistory: { summary: 'Older search note.', limits: 'Public sources only.', entries: [{ identifier: 'EXAMPLE-ADVISORY-001',
        impact: 'Malformed requests may bypass validation.', affectedVersions: '1.x', reference: 'https://example.test/advisory', notes: 'Applicability to 2.x is unknown.' }] },
      threatModel: { diagram: 'flowchart LR\n  request["Request"] --> parser["Parser"]',
        securityObjectives: ['Protect example state.', 'Preserve request integrity.'], risks: ['Unauthorized change.'], assumptions: ['Service enabled.'],
        invariants: 'Only valid requests reach dispatch.', sinks: 'State update.', mitigations: 'Validation before dispatch.',
        attackerControl: 'Request bytes.', trustBoundaries: 'Network to parser.',
        assetsAtRisk: 'Example state.', plausibleImpacts: 'Unauthorized change.' },
      coverage: { summary: 'Mapped input to sink.', gaps: 'Authenticated path remains.', opportunities: 'Review parser modes.',
        mappingComplete: true, sinksInvestigated: false, openQuestions: ['Which modes are enabled?'],
        unexploredConnections: ['Parser to storage'] }
    },
    progressBlocker: { reason: 'Test account required.', actorId: 'agent-example', setAt: '2026-01-01T00:03:00.000Z' },
    coverageState: { covered: false, unmetRequirements: ['sinks', 'open_questions', 'blocked'],
      unblockedFindingIds: ['Direct finding', 'Child finding'] }
  };
  const map: BealeWorkspaceSurfaceMap = { schemaVersion: 1, workspaceId: 'workspace-example',
    surfaces: surfaces.map((item) => item.id === selected.id ? selected : item),
    agents: [{ sessionId: 'session-example', sessionTitle: 'Example session', agentId: 'root', surfaceId: selected.id }] };
  const providerModelCatalog = [{ providerId: 'openai-codex' as const, providerName: 'OpenAI', models: [
    { id: 'gpt-example', name: 'GPT Example', reasoning: true, effortLevels: [], contextWindow: 0, maxTokens: 0 }
  ] }];
  const html = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: selected, map, providerModelCatalog, onClose: () => undefined
  }));
  expect(html).toContain('aria-label="Surface details"');
  expect(html).toContain('Back to Surfaces');
  expect(html).toContain('research-side-nested-header is-campaign-item');
  expect(html).toContain('research-side-nested-name" title="parser">parser</span>');
  expect(html).toContain('Surface status: Blocked');
  expect(html).toMatch(/title="parser">parser<\/span><span class="workspace-surface-details-status blocked"/);
  expect(html).toMatch(/<span>First touched by<\/span><span class="memory-catalog-item-author"><span class="memory-catalog-item-author-provider"><svg[^>]+aria-hidden="true"[^>]*>/);
  expect(html).toContain('memory-catalog-item-author-model">GPT Example</span>');
  expect(html).not.toContain('gpt-example</span>');
  expect(html).toContain('Updated <time dateTime="2026-01-01T00:04:00.000Z"');
  expect(html).toContain('role="tablist" aria-label="Surface detail views"');
  expect(html).toContain('role="tab" aria-selected="true"');
  expect(html).toContain('>Breakdown</button>');
  expect(html).toContain('>Recon</button>');
  expect(html).toContain('>Analysis</button>');
  expect(html).toContain('>Threat Model</button>');
  expect(html).not.toContain('>Bug History</button>');
  expect(html).not.toContain('>History</button>');
  expect(html).not.toContain('>Hierarchy</button>');
  expect(html).toContain('About');
  expect(html).not.toContain('>Path</h3>');
  expect(html).not.toContain('<h3>Open Questions</h3>');
  expect(html).not.toContain('<h3>Related Components</h3>');
  expect(html).not.toContain('Research context');
  expect(html).not.toContain('Remaining requirements');
  expect(html).not.toContain('What We Know');
  expect(html).not.toContain('Gaps');
  expect(html).not.toContain('Possible Leads');
  expect(html).not.toContain('Mapped input to sink.');
  expect(html).not.toContain('Authenticated path remains.');
  expect(html).not.toContain('Review parser modes.');
  expect(html).not.toContain('Which modes are enabled?');
  expect(html).not.toContain('Parser to storage');
  expect(html).toContain('Test account required.');
  expect(html).not.toContain('Findings Needing Resolution');
  expect(html).not.toContain('Attacker Control');
  expect(html).not.toContain('Request bytes.');
  expect(html).not.toContain('EXAMPLE-ADVISORY-001');
  expect(html).toContain('Example session');
  const memory = (id: string, type: string, surfaceId = selected.id, workspaceId = map.workspaceId) => ({
    id, type, title: id, surfaceId, status: 'confirmed', workspaces: [{ id: workspaceId, name: 'Example Workspace' }],
    duplicateOfMemoryId: null
  }) as AppServerMemoryNodeSummary;
  const threatModel = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: selected, map, initialView: 'Threat Model', onClose: () => undefined,
    onOpenMemory: () => undefined,
    memories: [memory('Example invariant memory', 'invariant'), memory('Example sink memory', 'sink'),
      memory('Example mitigation memory', 'mitigation'), memory('Wrong workspace memory', 'sink', selected.id, 'other-workspace'),
      { ...memory('Duplicate memory', 'invariant'), duplicateOfMemoryId: 'primary-memory' },
      { ...memory('Rejected memory', 'sink'), status: 'rejected' }]
  }));
  expect(threatModel).toContain('aria-label="Threat model diagram"');
  expect(threatModel).not.toContain('<h3>Diagram</h3>');
  expect(threatModel).toContain('Rendering diagram…');
  expect(threatModel).toContain('aria-label="Expand threat model diagram for parser"');
  for (const title of ['Security Objectives', 'Risks', 'Assumptions', 'Invariants', 'Sinks', 'Mitigations']) {
    expect(threatModel).toContain(`<h3>${title}</h3>`);
  }
  expect(threatModel).not.toContain('<h3>Risk Impact</h3>');
  expect(threatModel.indexOf('aria-label="Threat model diagram"')).toBeLessThan(threatModel.indexOf('<h3>Security Objectives</h3>'));
  expect(threatModel.indexOf('<h3>Security Objectives</h3>')).toBeLessThan(threatModel.indexOf('<h3>Risks</h3>'));
  expect(threatModel).toContain('<ul class="workspace-surface-threat-list"><li>Protect example state.</li><li>Preserve request integrity.</li></ul>');
  expect(threatModel).toContain('Protect example state.');
  expect(threatModel).toContain('Example invariant memory');
  expect(threatModel).toContain('Example sink memory');
  expect(threatModel).toContain('Example mitigation memory');
  expect(threatModel).toContain('aria-label="Open invariant Example invariant memory"');
  expect(threatModel).toContain('aria-label="Open sink Example sink memory"');
  expect(threatModel).toContain('aria-label="Open mitigation Example mitigation memory"');
  expect(threatModel.match(/class="workspace-surface-threat-open"/g)).toHaveLength(3);
  expect(threatModel).not.toContain('Wrong workspace memory');
  expect(threatModel).not.toContain('Duplicate memory');
  expect(threatModel).not.toContain('Rejected memory');
  expect(threatModel).toContain('Earlier Invariants');
  expect(threatModel).toContain('Only valid requests reach dispatch.');
  expect(threatModel).toContain('<h3>Earlier Context</h3>');
  expect(threatModel).toContain('Attacker Control');
  expect(threatModel).toContain('Request bytes.');
  expect(threatModel).not.toContain('EXAMPLE-ADVISORY-001');
  const proposal = (id: string, surfaceId: string | null, workspaceId = map.workspaceId) => ({
    id, title: id, surfaceId, workspaceId, projection: 'lead', duplicateOfClaimId: null, securityTracking: null
  }) as AppServerFindingSummary;
  const linkedTracking: NonNullable<AppServerFindingSummary['securityTracking']> = {
    reachability: { state: 'not_assessed', conditions: '', evidenceIds: [], assessorId: null,
      assessedAt: null, sourceRevision: null, environmentFingerprint: null },
    cvssAssessments: [], affectedAssetIds: [], surfaceIds: ['sink'], progressBlocker: null, externalReferences: []
  };
  const recon = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: selected, map, initialView: 'Recon', onClose: () => undefined,
    onOpenClaim: () => undefined,
    leads: [proposal('Assigned proposal', selected.id),
      { ...proposal('Refuted proposal', selected.id), status: 'rejected' },
      { ...proposal('Legacy linked proposal', null), securityTracking: { ...linkedTracking, surfaceIds: [selected.id] } },
      { ...proposal('Cross linked proposal', 'other-root'), securityTracking: { ...linkedTracking, surfaceIds: [selected.id] } },
      { ...proposal('Duplicate proposal', selected.id), duplicateOfClaimId: 'primary-claim' },
      proposal('Unrelated proposal', 'sink'),
      proposal('Other workspace proposal', selected.id, 'another-workspace')]
  }));
  expect(recon).toContain('<h3>Discovery</h3>');
  expect(recon).toContain('The example service exposes a request parser');
  expect(recon).toContain('<h3>Tech Stack</h3>');
  expect(recon).toContain('Example Parser');
  expect(recon).toContain('Synthetic package manifest');
  expect(recon).toContain('<h3>Assets</h3>');
  expect(recon.indexOf('<h3>Tech Stack</h3>')).toBeLessThan(recon.indexOf('<h3>Assets</h3>'));
  expect(recon.indexOf('<h3>Assets</h3>')).toBeLessThan(recon.indexOf('<h3>Bug History</h3>'));
  expect(recon).toContain('src/request_parser.ts');
  expect(recon).toContain('parseExampleRequest()');
  expect(recon).toContain('https://example.test/parser');
  for (const [status, label] of [['untouched', 'Untouched'], ['in_progress', 'In Progress'], ['complete', 'Complete']]) {
    expect(recon).toContain(`data-status="${status}">${label}</span>`);
  }
  expect(recon).toContain('<h3>Bug History</h3>');
  expect(recon).toContain('EXAMPLE-ADVISORY-001');
  expect(recon).toContain('Malformed requests may bypass validation.');
  expect(recon).toContain('Older search note.');
  expect(recon).toContain('<h3>Leads</h3>');
  expect(recon.indexOf('<h3>Bug History</h3>')).toBeLessThan(recon.indexOf('<h3>Leads</h3>'));
  expect(recon).toContain('Assigned proposal</button>');
  expect(recon).not.toContain('Refuted proposal');
  expect(recon).toContain('Legacy linked proposal</button>');
  expect(recon).toContain('Cross linked proposal</button>');
  expect(recon).not.toContain('Duplicate proposal');
  expect(recon).not.toContain('Unrelated proposal');
  expect(recon).not.toContain('Other workspace proposal');
  expect(recon).not.toContain('Review parser modes.');
  expect(recon).not.toContain('Request bytes.');
  const finding = (title: string, surfaceId: string | null, workspaceId = map.workspaceId) => ({
    id: title, title, surfaceId, workspaceId, projection: 'finding', duplicateOfClaimId: null,
    securityTracking: null, rating: 'high', maturity: 'observed'
  }) as AppServerFindingSummary;
  const analysis = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: selected, map, initialView: 'Analysis', onOpenClaim: () => undefined,
    onOpenMemory: () => undefined, onClose: () => undefined,
    findings: [finding('Direct finding', selected.id), finding('Child finding', 'sink'),
      { ...finding('Legacy linked finding', null), securityTracking: linkedTracking },
      { ...finding('Cross linked finding', 'other-root'), securityTracking: linkedTracking },
      { ...finding('Duplicate finding', selected.id), duplicateOfClaimId: 'primary-claim' },
      finding('Unrelated finding', 'other-root'), finding('Other workspace finding', selected.id, 'another-workspace'),
      { ...finding('Proposal only', selected.id), projection: 'lead' }],
    memories: [memory('Direct trajectory', 'trajectory'), memory('Child trajectory', 'trajectory', 'sink'),
      memory('Unrelated trajectory', 'trajectory', 'other-root'),
      memory('Other workspace trajectory', 'trajectory', selected.id, 'another-workspace'),
      { ...memory('Duplicate trajectory', 'trajectory'), duplicateOfMemoryId: 'primary-memory' },
      { ...memory('Rejected trajectory', 'trajectory'), status: 'rejected' },
      memory('Other memory type', 'invariant')]
  }));
  expect(analysis).toContain('<h3>Findings</h3>');
  expect(analysis).toContain('<h3>Findings Needing Resolution</h3><p>2 recorded</p>');
  expect(analysis).toContain('<h3>Trajectories</h3>');
  expect(analysis.indexOf('<h3>Findings</h3>')).toBeLessThan(analysis.indexOf('<h3>Findings Needing Resolution</h3>'));
  expect(analysis.indexOf('<h3>Findings Needing Resolution</h3>')).toBeLessThan(analysis.indexOf('<h3>Trajectories</h3>'));
  expect(analysis).toContain('Direct finding</button>');
  expect(analysis).toContain('Child finding</button>');
  expect(analysis).toContain('Legacy linked finding</button>');
  expect(analysis).toContain('Cross linked finding</button>');
  expect(analysis).toContain('High · Observed · sink');
  expect(analysis).toContain('Direct trajectory</button>');
  expect(analysis).toContain('Child trajectory</button>');
  expect(analysis).toContain('Confirmed · sink');
  for (const omitted of ['Duplicate finding', 'Unrelated finding', 'Other workspace finding', 'Proposal only']) {
    expect(analysis).not.toContain(omitted);
  }
  for (const omitted of ['Unrelated trajectory', 'Other workspace trajectory', 'Duplicate trajectory',
    'Rejected trajectory', 'Other memory type']) expect(analysis).not.toContain(omitted);
});

test('surface details keep a recorded first touch when an older server omits its author', () => {
  const map: BealeWorkspaceSurfaceMap = { schemaVersion: 1, workspaceId: 'workspace-example', surfaces, agents: [] };
  const touched = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: surfaces[1]!, map, onClose: () => undefined
  }));
  expect(touched).toContain('<span>First touched by</span><span class="memory-catalog-item-author">');
  expect(touched).toContain('memory-catalog-item-author-model">Unknown</span>');
  expect(touched).not.toContain('No first touch recorded');
  expect(touched).toContain('Last update unknown');

  const untouched = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: surfaces[0]!, map, onClose: () => undefined
  }));
  expect(untouched).toContain('No first touch recorded');
  expect(untouched).not.toContain('Open Questions');
  expect(untouched).not.toContain('Related Components');
  expect(renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: surfaces[0]!, map, initialView: 'Threat Model', onClose: () => undefined
  }))).toContain('No threat model diagram recorded.');
  expect(renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: surfaces[0]!, map, initialView: 'Recon', onClose: () => undefined
  }))).toContain('No research assets recorded.');
  const analysis = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: surfaces[0]!, map, initialView: 'Analysis', onClose: () => undefined
  }));
  expect(analysis).toContain('No findings recorded for this surface or its components.');
  expect(analysis).toContain('No trajectories recorded for this surface or its components.');
});

test('root surface Resources view exposes the linked resource form and checkout state', () => {
  const root: Surface = { ...surface('resource-example', null, null), resourceKind: 'repository',
    resourceSource: 'explicit_scope', resourceDirection: 'in_scope', resourceLocator: 'https://github.com/example-org/example-repository',
    scopeAssetId: 'asset-example' };
  const map: BealeWorkspaceSurfaceMap = { schemaVersion: 1, workspaceId: 'workspace-example', surfaces: [root], agents: [] };
  const html = renderToStaticMarkup(createElement(WorkspaceSurfaceDetails, {
    surface: root, map, workspaceId: 'workspace-example', initialView: 'Resources',
    activeScope: { id: 'scope-example', version: 1, status: 'active', workspaceName: 'Example Workspace',
      scopeOwner: 'ExampleCo', descriptionMarkdown: '', rulesMarkdown: '', activeFrom: '2026-01-01T00:00:00.000Z',
      expiresAt: null, createdAt: '2026-01-01T00:00:00.000Z', createdBy: 'user', assets: [{
        id: 'asset-example', scopeVersionId: 'scope-example', direction: 'in_scope', kind: 'repo',
        value: 'https://github.com/example-org/example-repository', sensitivity: 'public', attributes: {},
        createdAt: '2026-01-01T00:00:00.000Z'
      }] },
    onChangeResource: async () => undefined, onCloneRepository: async () => undefined, onClose: () => undefined
  }));
  expect(html).toContain('>Resources</button>');
  expect(html).toContain('aria-labelledby=');
  expect(html).toContain('>Reference</span>');
  expect(html).toContain('value="https://github.com/example-org/example-repository"');
  expect(html).toContain('>Cloned directory</span>');
  expect(html).toContain('Not cloned');
  expect(html).toContain('Clone repository');
  expect(html).toContain('Prior art');
});

test('blocked surfaces show their dependency in the map', () => {
  const reason = 'Pre-auth requests were tested; an authorized account is required for the remaining session path.';
  const blocked: Surface = { ...surfaces[1]!, progressBlocker: { reason, actorId: 'agent-example', setAt: '2026-01-01T00:03:00.000Z' },
    coverageState: { covered: false, unmetRequirements: ['blocked'], unblockedFindingIds: [] } };
  const map: BealeWorkspaceSurfaceMap = { schemaVersion: 1, workspaceId: 'workspace-example',
    surfaces: surfaces.map((item) => item.id === blocked.id ? blocked : item), agents: [] };
  const html = renderToStaticMarkup(createElement(WorkspaceSurfaceMap, { map, workspaceName: 'Example Workspace' }));
  expect(html).toContain('1 blocked');
  expect(html).toContain('workspace-surface-node blocked');
  expect(html).toContain('lucide-triangle-alert');
  expect(html).not.toContain('>Blocked</span>');
  expect(html).toContain(`Blocked: ${reason}`);
});

test('surface map keeps the workspace heading while loading', () => {
  const html = renderToStaticMarkup(createElement(WorkspaceSurfaceMap, { map: null, workspaceName: 'Example Workspace', loading: true }));
  expect(html).toContain('<h2>Example Workspace Surfaces</h2>');
  expect(html).toContain('aria-label="Search surfaces"');
  expect(html).toContain('Loading surfaces…');
});
