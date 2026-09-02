import { createElement } from 'react';
import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { ResearchChannelSummary, ResearchSessionSummary, WorkspaceRegistryEntry } from '@shared/types';
import {
  ArchiveSettingsView,
  SettingsSidebar,
  settingsSectionHeaderIcon,
  settingsSectionLabel
} from '../src/renderer/features/settings/SettingsModal';

describe('renderer archive settings', () => {
  it('adds Archive to Agent Settings and renders restorable sessions, Quick Chats, and channels', () => {
    const sidebar = renderToStaticMarkup(createElement(SettingsSidebar, {
      collapsed: false,
      section: 'archive',
      error: null,
      onBack: () => undefined,
      onChangeSection: () => undefined,
      onResizePointerDown: () => undefined
    }));
    const view = renderToStaticMarkup(createElement(ArchiveSettingsView, {
      sessions: [session],
      channels: [channel],
      quickChats: [quickChat],
      workspaces: [workspace],
      loading: false,
      onRestoreSession: async () => undefined,
      onRestoreChannel: async () => undefined,
      onResumeQuickChat: async () => undefined
    }));

    expect(settingsSectionLabel('archive')).toBe('Archive');
    expect(settingsSectionHeaderIcon('archive')).toBe('settings-archive');
    expect(sidebar).toContain('lucide-archive');
    expect(sidebar).toContain('<span>Archive</span>');
    expect(view).toContain('Archived Sessions');
    expect(view).toContain('Archived Channels');
    expect(view).toContain('Archived Quick Chats');
    expect(view).toContain('Parser investigation');
    expect(view).toContain('Review the latest parser results');
    expect(view).toContain('parser-review');
    expect(view).toContain('Example Workspace');
    expect(view.match(/>Restore</gu)).toHaveLength(2);
    expect(view.match(/>Resume</gu)).toHaveLength(1);
  });

  it('requires confirmation before sidebar archive callbacks run', () => {
    const source = readFileSync(new URL('../src/renderer/features/workspaces/WorkspaceSidebar.tsx', import.meta.url), 'utf8');
    expect(source).toContain('window.confirm(`Archive #${channel.name}?');
    expect(source).toContain('window.confirm(`Archive “${promptSessionTitle(session)}”?');
    expect(source.indexOf('window.confirm(`Archive #${channel.name}?'))
      .toBeLessThan(source.indexOf('void onArchiveChannel(channel)'));
    expect(source.indexOf('window.confirm(`Archive “${promptSessionTitle(session)}”?'))
      .toBeLessThan(source.indexOf('void onArchiveSession(session)'));
  });
});

const workspace = {
  id: 'registry_workspace', workspacePath: '/tmp/example', workspaceId: 'workspace_example', workspaceName: 'Example Workspace',
  researchProfileId: 'security-research', researchKitId: 'general', scopeOwner: 'Researcher', descriptionMarkdown: '',
  rulesMarkdown: '', expiresAt: null, createdAt: '2026-08-24T00:00:00.000Z', updatedAt: '2026-08-24T00:00:00.000Z',
  lastOpenedAt: null, runCount: 1, lastRunAt: '2026-08-24T00:00:00.000Z'
} satisfies WorkspaceRegistryEntry;

const session = {
  id: 'session_parser', registryWorkspaceId: workspace.id, workspacePath: workspace.workspacePath, workspaceId: workspace.workspaceId,
  runId: 'run_parser', title: 'Parser investigation', status: 'completed', runEngine: 'app-server', mode: 'dynamic',
  promptMarkdown: 'Investigate parser.', summary: 'Complete.', finalDisposition: null, model: 'gpt-5.6-sol', reasoningEffort: 'high',
  sandboxProfile: 'host', createdAt: '2026-08-24T00:00:00.000Z', startedAt: null, endedAt: null,
  updatedAt: '2026-08-24T00:00:00.000Z', resultViewedAt: null, archivedAt: '2026-08-24T01:00:00.000Z'
} satisfies ResearchSessionSummary;

const quickChat = {
  ...session,
  id: 'session_quick_chat',
  registryWorkspaceId: 'registry_quick_chats',
  workspacePath: '/tmp/internal-workspaces/quick-chats',
  workspaceId: 'workspace_quick_chats',
  runId: 'run_quick_chat',
  title: 'Review the latest parser results',
  mode: 'quick-chat',
  promptMarkdown: 'Review the latest parser results.',
  archivedAt: null
} satisfies ResearchSessionSummary;

const channel = {
  id: 'channel_parser', workspaceId: workspace.workspaceId, name: 'parser-review', title: 'Parser Review', topic: 'Review parser.',
  createdBySessionId: session.id, createdByAgentPath: '/root', createdAt: '2026-08-24T00:00:00.000Z',
  updatedAt: '2026-08-24T00:00:00.000Z', archivedAt: '2026-08-24T01:00:00.000Z', memberCount: 1,
  messageCount: 2, latestMessagePreview: 'Done.'
} satisfies ResearchChannelSummary;
