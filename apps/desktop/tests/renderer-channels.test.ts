import { createElement } from 'react';
import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { ResearchChannelDetail, ResearchChannelSummary } from '@shared/types';
import {
  ChannelWorkspace,
  channelSubagentName,
  channelTranscriptScrollEdges,
  formatChannelMessageTime
} from '../src/renderer/features/channels/ChannelWorkspace';
import { INSET_SCROLLBAR_SELECTOR } from '../src/renderer/hooks/useInsetScrollbarActivation';
import { WorkspaceSidebar } from '../src/renderer/features/workspaces/WorkspaceSidebar';
import { canonicalResearchChannelName, normalizeResearchChannelNameDraft } from '../src/renderer/view-models/researchChannels';

const channel: ResearchChannelSummary = {
  id: 'channel_parser',
  workspaceId: 'workspace_one',
  name: 'parser-review',
  title: 'Parser review',
  topic: 'Carry parser boundary research across sessions.',
  createdBySessionId: 'session_one',
  createdByAgentPath: '/root',
  createdAt: '2026-08-23T12:00:00.000Z',
  updatedAt: '2026-08-24T12:00:00.000Z',
  memberCount: 2,
  messageCount: 1,
  latestMessagePreview: 'The allocation omits a terminator.'
};

describe('research channels', () => {
  it('preserves the first channel selection while leaving a session for the workspace view', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const resetStart = appSource.indexOf('useEffect(() => {\n    setSelectedChannelId(null);');
    const catalogStart = appSource.indexOf('useEffect(() => {', resetStart + 1);
    const resetEffect = appSource.slice(resetStart, catalogStart);
    const openStart = appSource.indexOf('const openResearchChannel = useCallback');
    const openEnd = appSource.indexOf('const createResearchChannel = useCallback', openStart);
    const openAction = appSource.slice(openStart, openEnd);

    expect(resetStart).toBeGreaterThan(-1);
    expect(resetEffect).toContain('}, [snapshot?.workspace.workspaceId]);');
    expect(resetEffect).not.toContain('selectedRunId');
    expect(openAction.indexOf('setSelectedRunId(null);')).toBeLessThan(openAction.indexOf('setSelectedChannelId(channel.id);'));
    expect(openAction).toContain('setSelectedChannelDetail(null);');
  });

  it('switches the left sidebar to a selected workspace channel list', () => {
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: { registryPath: '/tmp/workspaces.json', workspaces: [], researchSessions: [] },
      selectedRunId: null,
      snapshot: null,
      channels: [channel],
      selectedChannelId: channel.id,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onOpenChannel: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));

    expect(html).toContain('role="tab" aria-selected="false"');
    expect(html).toContain('>Workspaces</button>');
    expect(html).toContain('role="tab" aria-selected="true" class="active">Channels</button>');
    expect(html).toContain('class="sidebar-channel-group" role="group" aria-label="All Channels"');
    expect(html).toContain('<span>All Channels</span>');
    expect(html.match(/lucide-hash/gu)).toHaveLength(1);
    expect(html).toContain('class="sidebar-channel-indent"');
    expect(html).toContain('parser-review');
    expect(html).not.toContain('The allocation omits a terminator.');
    expect(html).not.toContain('Carry parser boundary research across sessions.');
    expect(html).toContain('aria-current="page"');
    expect(html).toContain('aria-label="Archive channel parser-review"');
    expect(html).toContain('lucide-archive');
  });

  it('shows four channels before an animated Show more overflow', () => {
    const channels = Array.from({ length: 6 }, (_, index): ResearchChannelSummary => ({
      ...channel,
      id: `channel_${index}`,
      name: `channel-${index}`,
      title: `Channel ${index}`
    }));
    const html = renderToStaticMarkup(createElement(WorkspaceSidebar, {
      busy: false,
      collapsed: false,
      error: null,
      workspaceRegistry: { registryPath: '/tmp/workspaces.json', workspaces: [], researchSessions: [] },
      selectedRunId: null,
      snapshot: null,
      channels,
      selectedChannelId: channels[0].id,
      onAddWorkspace: () => undefined,
      onImportWorkspace: () => undefined,
      onOpenWorkspace: () => undefined,
      onOpenResearchSession: () => undefined,
      onOpenChannel: () => undefined,
      onResizePointerDown: () => undefined,
      onStartNewResearch: () => undefined,
      onStartNewResearchForWorkspace: () => undefined
    }));
    const source = readFileSync(new URL('../src/renderer/features/workspaces/WorkspaceSidebar.tsx', import.meta.url), 'utf8');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');

    expect(source).toContain('const SIDEBAR_CHANNEL_LIMIT = 4;');
    expect(source).toContain("channelsExpanded ? 'Show less' : `Show ${hiddenChannels.length} more`");
    expect(html).toContain('class="workspace-session-overflow" aria-hidden="true" inert=""');
    expect(html).toContain('class="workspace-session-overflow-inner"');
    expect(html).toContain('class="session-memory-type-toggle" aria-expanded="false">Show 2 more</button>');
    expect(styles).toMatch(/\.workspace-session-overflow\s*\{[^}]*grid-template-rows:\s*0fr;[^}]*transition:\s*grid-template-rows 180ms ease;/s);
    expect(styles).toMatch(/\.workspace-session-overflow\.expanded\s*\{[^}]*grid-template-rows:\s*1fr;/s);
  });

  it('normalizes channel-name input to at most three lowercase dash-separated words', () => {
    expect(normalizeResearchChannelNameDraft('Parser Review ')).toBe('parser-review-');
    expect(normalizeResearchChannelNameDraft('Parser Review Evidence Notes')).toBe('parser-review-evidence');
    expect(canonicalResearchChannelName('  Parser / Review / Evidence  ')).toBe('parser-review-evidence');
  });

  it('formats channel sender names and compact message times', () => {
    expect(channelSubagentName('/root/parser_review')).toBe('Parser Review');
    expect(formatChannelMessageTime(new Date(2026, 7, 23, 16, 32).toISOString())).toBe('4:32p');
  });

  it('shows channel transcript shadows only where more scroll content exists', () => {
    expect(channelTranscriptScrollEdges({ scrollHeight: 500, clientHeight: 200, scrollTop: 0 }))
      .toEqual({ top: false, bottom: true });
    expect(channelTranscriptScrollEdges({ scrollHeight: 500, clientHeight: 200, scrollTop: 120 }))
      .toEqual({ top: true, bottom: true });
    expect(channelTranscriptScrollEdges({ scrollHeight: 500, clientHeight: 200, scrollTop: 300 }))
      .toEqual({ top: true, bottom: false });
    expect(channelTranscriptScrollEdges({ scrollHeight: 200, clientHeight: 200, scrollTop: 0 }))
      .toEqual({ top: false, bottom: false });
  });

  it('renders durable cross-session messages and a human composer', () => {
    const detail: ResearchChannelDetail = {
      channel,
      members: [{
        id: 'member_parser', channelId: channel.id, sessionId: 'session_old', agentId: 'agent_old',
        agentPath: '/root/parser', provider: 'openai-codex', model: 'gpt-5.6-sol', role: 'reviewer',
        status: 'completed',
        joinedAt: '2026-08-23T12:00:00.000Z', lastSeenAt: '2026-08-23T12:01:00.000Z'
      }, {
        id: 'member_explorer', channelId: channel.id, sessionId: 'session_live', agentId: 'agent_live',
        agentPath: '/root/explorer', provider: 'openai-codex', model: 'gpt-5.6-sol', role: 'researcher',
        status: 'running',
        joinedAt: '2026-08-24T12:00:00.000Z', lastSeenAt: '2026-08-24T12:01:00.000Z'
      }],
      messages: [{
        id: 'message_parser', channelId: channel.id, sessionId: 'session_old', attemptId: 'attempt_old', memberId: 'member_parser',
        senderAgentPath: '/root/parser', kind: 'evidence', contentMarkdown: 'The allocation omits a terminator.',
        evidenceRefs: ['code:parser:44'], metadata: {}, createdAt: '2026-08-23T12:01:00.000Z'
      }, {
        id: 'message_runbook', channelId: channel.id, sessionId: 'session_old', attemptId: 'attempt_old', memberId: 'member_parser',
        senderAgentPath: '/root/parser', kind: 'message', contentMarkdown: 'The reproducer is ready.',
        evidenceRefs: [], metadata: { source: 'channel_share', sharedResourceKind: 'runbook' }, createdAt: '2026-08-23T12:02:00.000Z'
      }],
      sharedResources: [{
        id: 'shared_runbook', channelId: channel.id, sessionId: 'session_old', memberId: 'member_parser',
        messageId: 'message_runbook', senderAgentPath: '/root/parser', kind: 'runbook', resourceId: 'runbook_parser',
        title: 'Parser reproducer', createdAt: '2026-08-23T12:02:00.000Z', updatedAt: '2026-08-23T12:02:00.000Z'
      }]
    };
    const html = renderToStaticMarkup(createElement(ChannelWorkspace, {
      detail,
      loading: false,
      error: null,
      posting: false,
      providerModelCatalog: [{
        providerId: 'openai-codex',
        providerName: 'OpenAI',
        models: [{
          id: 'gpt-5.6-sol',
          name: 'GPT-5.6 Sol',
          reasoning: true,
          effortLevels: ['high'],
          contextWindow: 256_000,
          maxTokens: 32_000
        }]
      }],
      onRefresh: () => undefined,
      onPost: async () => undefined,
      onDelete: async () => undefined
    }));

    expect(html).not.toContain('Carry parser boundary research across sessions.');
    expect(html).toContain('The allocation omits a terminator.');
    expect(html).toContain('<span class="channel-message-sender">Parser</span>');
    expect(html).toContain('<span class="channel-message-kind">Evidence</span>');
    expect(html).toContain('class="channel-message-provider-icon"');
    expect(html).toContain('<span>5.6 Sol</span>');
    expect(html).not.toContain('>EVIDENCE</span>');
    expect(html).toContain('Evidence: code:parser:44');
    expect(html).toContain('placeholder="Message #parser-review"');
    expect(html).toContain('class="channel-transcript-scroll"');
    expect(html).toContain('class="channel-composer-destination"');
    expect(html).toContain('main-steer-send channel-composer-send');
    expect(html).toContain('lucide-arrow-right');
    expect(html).toContain('aria-label="Channel summary"');
    expect(html).toContain('>Members</h2>');
    expect(html).toContain('>Shared</h2>');
    expect(html).toContain('>Files</span><strong>0</strong>');
    expect(html).toContain('>Runbooks</span><strong>1</strong>');
    expect(html).toContain('>Memories</span><strong>0</strong>');
    expect(html).toContain('class="channel-message-shared-resource"');
    expect(html).toContain('Shared a runbook');
    expect(html).toContain('aria-label="Status: Completed"');
    expect(html).toContain('class="lucide lucide-loader-circle channel-summary-member-spinner" aria-label="Status: Running"');
    expect(html).toContain('class="channel-summary-member is-inactive" title="Open Parser details"');
    expect(html).toContain('>Parser</span>');
    expect(html).not.toContain('lucide-bot');
    expect(html).not.toContain('>Completed</span>');
    expect(html).toContain('class="channel-summary-member-age" dateTime="2026-08-23T12:02:00.000Z"');
    expect(html).toContain('channel-summary-member-chevron');
    expect(html).toContain('lucide-chevron-right');
  });

  it('uses the session content background for the channel main pane', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const channelWorkspaceStyles = styles.match(/\.channel-workspace\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelTranscriptStyles = styles.match(/\.channel-transcript\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelComposerStyles = styles.match(/\.channel-composer\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelSummaryMemberStyles = styles.match(/\.channel-summary-member\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelSummaryPanelStyles = styles.match(/\.channel-summary-panel\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelSummarySectionStyles = styles.match(/\.channel-summary-section\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelMessageSenderStyles = styles.match(/\.channel-message-sender\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelMessageTimeStyles = styles.match(/\.channel-message-content > header time\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelMessageBodyStyles = styles.match(/\.channel-message-body\s*\{([^}]*)\}/u)?.[1] ?? '';
    const channelSidebarItemStyles = styles.match(/\.sidebar-channel-item\s*\{([^}]*)\}/u)?.[1] ?? '';
    expect(channelWorkspaceStyles).toContain('background: var(--panel);');
    expect(channelWorkspaceStyles).toContain('border-radius: 0 var(--content-surface-radius) var(--content-surface-radius) 0;');
    expect(channelWorkspaceStyles).toContain('overflow: hidden;');
    expect(channelWorkspaceStyles).not.toContain('var(--gray-08)');
    expect(channelTranscriptStyles).toContain('padding: 8px;');
    expect(INSET_SCROLLBAR_SELECTOR).toContain('.channel-transcript');
    expect(styles).toMatch(/:where\([\s\S]*?\.channel-transcript[\s\S]*?\)\s*\{\s*scrollbar-color: transparent transparent;/u);
    expect(channelComposerStyles).toContain('width: min(920px, calc(100% - 16px));');
    expect(channelComposerStyles).toContain('margin: 0 auto 8px;');
    expect(channelComposerStyles).toContain('background: var(--panel-raised);');
    expect(channelComposerStyles).toContain('border-radius: var(--content-surface-radius);');
    expect(channelSummaryMemberStyles).toContain('min-height: 30px;');
    expect(channelSummaryMemberStyles).toContain('line-height: 1.3;');
    expect(channelSummaryMemberStyles).toContain('grid-template-columns: 14px minmax(0, 1fr) auto 16px;');
    expect(styles).toMatch(/\.channel-summary-member-spinner\s*\{[^}]*animation: workspace-startup-spin 900ms linear infinite;/u);
    expect(channelSummaryPanelStyles).toContain('padding-bottom: 12px;');
    expect(channelSummarySectionStyles).toContain('max-height: 50%;');
    expect(channelSummarySectionStyles).toContain('flex: 0 1 auto;');
    expect(channelSummarySectionStyles).toContain('overflow: hidden;');
    expect(channelMessageSenderStyles).toContain('font-weight: 400;');
    expect(channelMessageTimeStyles).toContain('margin-left: auto;');
    expect(channelMessageBodyStyles).toContain('color: var(--text);');
    expect(channelSidebarItemStyles).toContain('border-radius: 8px;');
    expect(styles).toMatch(/\.sidebar-list-scroll \.workspace-item-row,\s*\.sidebar-list-scroll \.sidebar-channel-item,\s*\.sidebar-list-scroll \.workspace-session-item\s*\{[^}]*width: 100%;[^}]*margin-inline: 0;/u);
    expect(styles).toMatch(/\.channel-message-model\s*\{[^}]*color: var\(--muted-strong\);/u);
    expect(styles).toMatch(/\.channel-summary-member\.is-inactive[\s\S]*?color: var\(--muted\);/u);
    expect(styles).toMatch(/\.sidebar-channel-group-heading\s*\{[^}]*padding: 6px 30px 6px 4px;/u);
    expect(styles).toMatch(/\.sidebar-channel-group-heading > svg\s*\{[^}]*transform: translateX\(2px\);/u);
    expect(styles).toMatch(/\.sidebar-row-archive-button\s*\{[\s\S]*?opacity: 0;/u);
    expect(styles).toMatch(/\.sidebar-channel-row:hover \.workspace-session-age[\s\S]*?visibility: hidden;/u);
  });
});
