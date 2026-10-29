import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, test } from 'vitest';
import type { BealeAppServerWorkspaceDetails, FleetRemoteCatalog } from '@beale/app-server-runtime/protocol';
import { RemoteFleetSidebar, RemoteFleetWorkspace } from '../src/renderer/features/fleet/RemoteFleetWorkspace';
import { RemoteWorkspaceDetails } from '../src/renderer/features/fleet/RemoteWorkspaceDetails';
import { RemoteQuickChatDock, remoteQuickChatModels } from '../src/renderer/features/fleet/RemoteQuickChatDock';
import { CommentaryView } from '../src/renderer/features/commentary/CommentaryView';
import { mergeRemoteSessionHistory, remoteRunDetail, remoteSessionHistoryPage, remoteWorkspaceRegistry } from '../src/renderer/features/fleet/remoteFleetProjection';
import { buildTraceDisplayEventsForAgentPath } from '../src/renderer/view-models/traceDisplay';
import { commentaryMessagesForSession } from '../src/renderer/view-models/commentary';
import { sessionDurationTiming } from '../src/renderer/view-models/sessionHeader';

const catalog: FleetRemoteCatalog = {
  serverId: 'server-example',
  providerCatalog: { controlVersion: 1, defaultProviderId: 'openai-codex', providers: [{ providerId: 'openai-codex', providerName: 'OpenAI', riskAcknowledged: true, defaultLeadModel: 'gpt-example', defaultSubagentModel: null, defaultReasoningEffort: 'high', models: [{ id: 'gpt-example', name: 'Example model', reasoning: true, effortLevels: ['high'] }] }] },
  workspaces: [{ id: 'registry-example', workspaceId: 'workspace-example', name: 'ExampleCo Research', runCount: 1 }],
  sessions: [{ id: 'session-example', workspaceId: 'workspace-example', title: 'Inspect example service', status: 'running', prompt: 'Inspect example.test', updatedAt: '2026-01-01T00:00:00.000Z', automation: false }],
  machines: [],
  browsers: [{ id: 'in-agent', name: 'In-Agent Browser', kind: 'embedded', enabled: true }],
  workspaceTargets: { 'workspace-example': ['local', 'browser:in-agent'] },
  requiredWorkspaceIds: [],
  optionalWorkspaceIds: [],
  hasBaseVm: false
};

test('remote workspaces and sessions use the standard sidebar hierarchy', () => {
  const registry = remoteWorkspaceRegistry(catalog, catalog.serverId);
  expect(registry?.workspaces.map((workspace) => workspace.workspaceName)).toEqual(['ExampleCo Research']);
  expect(registry?.researchSessions.map((session) => session.status)).toEqual(['active']);
  const html = renderToStaticMarkup(createElement(RemoteFleetSidebar, {
    catalog,
    serverName: 'Example server',
    selectedWorkspaceId: 'workspace-example',
    selectedSessionId: 'session-example',
    automations: false,
    collapsed: false,
    serverSelector: null,
    onSelect: () => undefined,
    onStartNewResearch: () => undefined,
    onOpenQuickChat: () => undefined,
    onResizePointerDown: () => undefined
  }));
  expect(html).toContain('workspace-list');
  expect(html).toContain('workspace-session-list');
  expect(html).toContain('ExampleCo Research');
  expect(html).toContain('Inspect example service');
  expect(html).toContain('Quick Chat');
  expect(html).not.toContain('sidebar-surface-slot');
});

test('remote workspace details render the workspace dashboard from the shared details DTO', () => {
  const details: BealeAppServerWorkspaceDetails = {
    schemaVersion: 1,
    workspace: { id: 'registry-example', workspaceId: 'workspace-example', name: 'ExampleCo Research', researchProfileId: 'security-research', researchKitId: 'general', runCount: 1, lastRunAt: null, updatedAt: '2026-01-01T00:00:00.000Z' },
    scope: { id: 'scope-example', name: 'ExampleCo Research', owner: 'ExampleCo', descriptionMarkdown: 'Research **example.test** only.', expiresAt: null, resources: [{ id: 'resource-example', kind: 'domain', direction: 'in_scope', value: 'example.test', sensitivity: 'public' }] },
    researchSubject: 'ExampleCo', rules: [{ id: 'rule-example', text: 'Stay within example.test.' }],
    memory: { schemaVersion: 4, workspaceId: 'workspace-example', status: 'ready', nodeCount: 0, nodeTypeCounts: {}, nodes: [], leads: [], findings: [] },
    runbooks: [{ id: 'runbook-example', title: 'Example runbook', purpose: 'Verify example.test.', updatedAt: '2026-01-01T00:00:00.000Z' }],
    reports: [{ id: 'report-example', title: 'Example report', summary: 'Synthetic report.', status: 'complete', updatedAt: '2026-01-01T00:00:00.000Z' }],
    surfaceMap: { schemaVersion: 1, workspaceId: 'workspace-example', surfaces: [{
      id: 'resource-example', parentId: null, resourceId: 'resource-example', name: 'example.test', description: '',
      resourceKind: 'domain', resourceSource: 'explicit_scope', resourceDirection: 'in_scope',
      resourceLocator: 'example.test', scopeAssetId: 'resource-example', firstTouchedAt: null,
      foundation: null, progressBlocker: null,
      coverageState: { covered: false, unmetRequirements: ['foundation'], unblockedFindingIds: [] }
    }], agents: [] },
    targets: [{ id: 'local', name: 'Local', ready: true }]
  };
  const html = renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details }));
  expect(html).toContain('aria-label="Workspace dashboard"');
  expect(html).toContain('main-session-grid workspace-context workspace-main-only');
  expect(html).toContain('research-side-column');
  expect(html).not.toContain('role="dialog"');
  expect(html).not.toContain('Open Browser');
  expect(html).toContain('Campaign');
  expect(html).not.toContain('<span>Resources</span>');
  expect(html).toContain('Surfaces');
  expect(html).toContain('Rules');
  expect(html).toContain('Targets');
  expect(html).toContain('Settings');
  expect(html).toContain('Runbooks');
  expect(html).toContain('Reports');
  expect(html).toContain('ExampleCo Research Findings');
  expect(html).toContain('campaign-board-lanes');
  expect(html).toContain('aria-label="Search findings"');
  expect(html).not.toContain('remote-workspace-details-panel');
  expect(html).not.toContain('New Research prompt');

  const settings = renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details, initialView: 'overview' }));
  expect(settings).toContain('workspace-overview-layout');
  expect(settings).toContain('aria-label="Workspace Name"');
  expect(settings).not.toContain('workspace-removal-form');
  expect(settings).not.toContain('workspace-directories-field');

  const resources = renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details, initialView: 'resources' }));
  expect(resources).toContain('workspace-surface-node');
  expect(resources).toContain('example.test');
  expect(resources).not.toContain('Add resource type');

  const surfaces = renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details, initialView: 'surfaces' }));
  expect(surfaces).toContain('aria-label="Workspace surfaces"');

  const targets = renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details, initialView: 'targets' }));
  expect(targets).toContain('workspace-targets-panel');
  expect(targets).toContain('<h2>ExampleCo Research Targets</h2>');
  expect(targets).toContain('Selected on this app server');

  const populated: BealeAppServerWorkspaceDetails = { ...details, memory: { ...details.memory,
    nodeCount: 1, nodeTypeCounts: { observation: 1 },
    nodes: [{ id: 'memory-example', sessionIds: [], type: 'observation', title: 'Example observation',
      summary: 'Observed on example.test.', status: 'confirmed', confidence: 0.8, tags: [],
      createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z', revision: 1, duplicateMemories: [] }],
    findings: [{ id: 'claim-example', sessionIds: [], projection: 'finding', maturity: 'observed',
      status: 'observed', staleFromStatus: null, rating: 'low', classification: 'example.class',
      duplicateClaims: [], title: 'Example claim', summary: 'Synthetic claim.',
      impact: 'Example impact.', stepsToReproduce: '', securityTracking: null, confidence: 0.7, evidenceCount: 0,
      createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:00:00.000Z', revision: 1 }]
  } };
  expect(renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details: populated }))).toContain('Example claim');
  expect(renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details: populated, initialView: 'memory' }))).toContain('Example observation');
  expect(renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details: populated, initialView: 'runbooks' }))).toContain('Example runbook');
  expect(renderToStaticMarkup(createElement(RemoteWorkspaceDetails, { serverId: 'server-example', details: populated, initialView: 'reports' }))).toContain('Example report');
});

test('remote workspace selection and New Research have distinct views', () => {
  const props = {
    serverId: 'server-example', serverName: 'Example server', catalog,
    workspaceId: 'workspace-example', sessionId: null, automations: false,
    error: null, onLaunched: () => undefined
  };
  const details = renderToStaticMarkup(createElement(RemoteFleetWorkspace, { ...props, newResearchOpen: false }));
  const newResearch = renderToStaticMarkup(createElement(RemoteFleetWorkspace, { ...props, newResearchOpen: true }));
  expect(details).toContain('Loading workspace details');
  expect(details).not.toContain('New Research prompt');
  expect(newResearch).toContain('new-research-options-tray');
  expect(newResearch).toContain('Example server');
  expect(newResearch).toContain('Research targets: Local, ready');
  expect(newResearch).toContain('Research suggestion categories');
});

test('remote Quick Chat opens in the dock without selecting a workspace', () => {
  const html = renderToStaticMarkup(createElement(RemoteQuickChatDock, {
    serverId: 'server-example', catalog, chats: [{ id: 'quick-chat-example', serverId: 'server-example' }],
    onClose: () => undefined, onStarted: () => undefined
  }));
  expect(html).toContain('aria-label="Quick chats"');
  expect(html).toContain('aria-label="Resize quick chat"');
  expect(html).toContain('Send a quick chat message');
  expect(html).toContain('Any topic, any workspace');
  expect(html).toContain('This chat will be automatically archived.');
  expect(html).not.toContain('Recent Quick Chats');
  expect(remoteQuickChatModels(catalog).initialSelection).toEqual({
    provider: 'openai-codex', model: 'gpt-example', reasoningEffort: 'high'
  });
});

test('remote session duration excludes time between active periods', () => {
  const detail = remoteRunDetail({ result: {
    session: {
      id: 'session-example', status: 'completed', title: 'Example session', prompt: 'Inspect example.test',
      model: 'gpt-example', createdAt: '2026-01-01T00:00:00.000Z',
      endedAt: '2026-01-03T00:05:00.000Z', updatedAt: '2026-01-03T00:05:00.000Z', metadata: {},
      activityIntervals: [
        { startedAt: '2026-01-01T00:00:00.000Z', endedAt: '2026-01-01T00:10:00.000Z' },
        { startedAt: '2026-01-03T00:00:00.000Z', endedAt: '2026-01-03T00:05:00.000Z' }
      ]
    }, events: []
  } });
  expect(detail?.activityIntervals).toHaveLength(2);
  expect(sessionDurationTiming(detail!, Date.parse('2026-01-03T00:05:00.000Z'))?.durationMs).toBe(15 * 60_000);
});

test('remote canonical events feed the local commentary projection', () => {
  const detail = remoteRunDetail({ result: {
    session: { id: 'session-example', status: 'running', title: 'Inspect example service', prompt: 'Inspect example.test', model: 'example-model', createdAt: '2026-01-01T00:00:00.000Z', updatedAt: '2026-01-01T00:01:00.000Z', metadata: {} },
    events: [
      { id: 'event-user', kind: 'beale.transcript', timestamp: '2026-01-01T00:00:00.000Z', payload: { record: { id: 'user-example', role: 'user', contentMarkdown: 'Inspect example.test', source: 'run_prompt', metadata: {}, createdAt: '2026-01-01T00:00:00.000Z' } } },
      { id: 'event-commentary', kind: 'model.output', timestamp: '2026-01-01T00:00:30.000Z', payload: { phase: 'completed', messagePhase: 'commentary', text: '**Checking** the service.', agentPath: '/root' } },
      { id: 'event-tool', kind: 'research.event', timestamp: '2026-01-01T00:00:40.000Z', payload: { event: { kind: 'tool.requested', agentPath: '/root', payload: { toolName: 'file.read', toolActionId: 'tool-example' } } } },
      { id: 'event-final', kind: 'model.output', timestamp: '2026-01-01T00:01:00.000Z', payload: { phase: 'completed', messagePhase: 'final_answer', text: 'Review complete.', agentPath: '/root' } }
    ]
  } });
  expect(detail).not.toBeNull();
  const events = buildTraceDisplayEventsForAgentPath(detail!, null);
  const messages = commentaryMessagesForSession(detail, events);
  expect(messages.map((message) => message.kind)).toEqual(['user', 'commentary', 'tool', 'final_answer']);
  expect(messages.map((message) => message.contentMarkdown)).toContain('**Checking** the service.');
  const html = renderToStaticMarkup(createElement(CommentaryView, {
    busy: false,
    detail,
    events,
    providerModelCatalog: [],
    selectedRunId: 'session-example',
    showBackToMain: false,
    searchHighlightQuery: '',
    onBackToMain: () => undefined,
    onSessionAction: () => undefined,
    onSteerInstruction: () => undefined
  }));
  expect(html).toContain('main-commentary-view');
  expect(html).toContain('<strong>Checking</strong>');
});

test('older remote pages merge with the live tail without repeating events', () => {
  const update = { result: { session: { id: 'session-example' }, events: [{ id: 'event-2', kind: 'research.event' }, { id: 'event-3', kind: 'research.event' }], hasEarlier: true } };
  const page = remoteSessionHistoryPage({ result: { events: [{ id: 'event-1', kind: 'research.event' }, { id: 'event-2', kind: 'research.event' }], hasEarlier: false } });
  expect(page).not.toBeNull();
  const merged = mergeRemoteSessionHistory(update, [page!]);
  expect((merged as typeof update).result.events.map((event) => event.id)).toEqual(['event-1', 'event-2', 'event-3']);
  expect(remoteRunDetail(merged)?.historyCursor).toEqual({ beforeEventId: 'event-1', hasEarlier: false });
});
