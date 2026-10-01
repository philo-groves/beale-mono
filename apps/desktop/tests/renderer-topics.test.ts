import { createElement } from 'react';
import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { ResearchTopicDetail, ResearchTopicSummary, WorkspaceRegistryEntry } from '@shared/types';
import { TopicWorkspace } from '../src/renderer/features/topics/TopicWorkspace';
import { TopicsExplorer, TopicsSidebar, topicsForScope } from '../src/renderer/features/topics/TopicsExplorer';
import { canonicalResearchTopicName } from '../src/renderer/view-models/researchTopics';

const topic: ResearchTopicSummary = {
  id: 'topic_example', workspaceId: 'workspace_example', name: 'parser-review', title: 'Parser review',
  topic: 'Track ExampleCo parser boundaries.', overviewMarkdown: 'Current understanding cites claim_example_001.',
  createdBySessionId: 'session_example', createdByAgentPath: '/root',
  createdAt: '2026-08-23T12:00:00.000Z', updatedAt: '2026-08-24T12:00:00.000Z',
  memberCount: 1, messageCount: 1, latestMessagePreview: 'Historical observation.'
};

const detail: ResearchTopicDetail = {
  topic,
  members: [],
  messages: [],
  sharedResources: [],
  pages: [{ id: 'page_example', topicId: topic.id, title: 'Open questions', contentMarkdown: 'Which sizes remain?', createdAt: topic.createdAt, updatedAt: topic.updatedAt }],
  links: [{ id: 'link_example', topicId: topic.id, kind: 'claim', resourceId: 'claim_example_001', title: 'Example hypothesis', createdAt: topic.createdAt }],
  mergedTopics: []
};

const callbacks = {
  onRefresh: () => undefined,
  onUpdateOverview: async () => undefined,
  onSavePage: async () => undefined,
  onDeletePage: async () => undefined,
  onLink: async () => undefined,
  onUnlink: async () => undefined,
  onArchive: async () => undefined,
  onMerge: async () => undefined,
  onUnmerge: async () => undefined,
  onOpenTopic: () => undefined,
  availableTopics: []
};

describe('research topics', () => {
  it('shows the topic overview and linked-record count before historical activity', () => {
    const html = renderToStaticMarkup(createElement(TopicWorkspace, {
      detail, loading: false, error: null, saving: false, ...callbacks
    }));
    expect(html).toContain('Parser review');
    expect(html).toContain('Current understanding cites claim_example_001.');
    expect(html).toContain('References (1)');
    expect(html).toContain('Pages (1)');
    expect(html).toContain('Activity history');
    expect(html).not.toContain('Historical observation.');
    expect(html).not.toContain('Post to topic');
  });

  it('shows recent topics in their dedicated sidebar and filters the explorer by workspace', () => {
    const workspaces = [
      { id: 'registry_example', workspaceId: 'workspace_example', workspaceName: 'Example Workspace' },
      { id: 'registry_other', workspaceId: 'workspace_other', workspaceName: 'Other Workspace' }
    ] as WorkspaceRegistryEntry[];
    const otherTopic = { ...topic, id: 'topic_other', workspaceId: 'workspace_other', title: 'Other review', updatedAt: '2026-08-25T12:00:00.000Z' };
    const onNavigate = () => undefined;
    const sidebar = renderToStaticMarkup(createElement(TopicsSidebar, {
      topics: [topic, otherTopic], workspaces, workspaceRegistryLoading: false, selectedWorkspaceId: null, activeWorkspaceId: topic.workspaceId,
      selectedTopicId: topic.id, collapsed: false, loading: false, error: null,
      onOpenExplorer: onNavigate, onOpenTopic: onNavigate, onCreateTopic: async () => undefined, onAddWorkspace: onNavigate,
      onResizePointerDown: onNavigate
    }));
    expect(sidebar).toContain('class="collection-sidebar-list-heading">Recent</div>');
    expect(sidebar).toContain('Parser review');
    expect(sidebar).toContain('Other review');
    expect(sidebar).toContain('aria-current="page"');
    const noWorkspaceSidebar = renderToStaticMarkup(createElement(TopicsSidebar, {
      topics: [], workspaces, workspaceRegistryLoading: false, selectedWorkspaceId: null, activeWorkspaceId: null,
      selectedTopicId: null, collapsed: false, loading: false, error: null,
      onOpenExplorer: onNavigate, onOpenTopic: onNavigate, onCreateTopic: async () => undefined, onAddWorkspace: onNavigate,
      onResizePointerDown: onNavigate
    }));
    expect(noWorkspaceSidebar).toContain('aria-haspopup="menu" aria-expanded="false"');
    expect(noWorkspaceSidebar).not.toContain('disabled=""');
    const explorer = renderToStaticMarkup(createElement(TopicsExplorer, {
      topics: [topic, otherTopic], workspaces, selectedWorkspaceId: topic.workspaceId,
      loading: false, error: null, onScopeChange: onNavigate, onOpenTopic: onNavigate
    }));
    expect(explorer).toContain('All Topics');
    expect(explorer).toContain('Example Workspace');
    expect(explorer).toContain('Other Workspace');
    expect(explorer).toContain('Parser review');
    expect(explorer).not.toContain('Other review');
    expect(topicsForScope([topic, otherTopic], null)[0]?.id).toBe('topic_other');
  });

  it('shows reversible merged topics without adding their old activity to the overview', () => {
    const html = renderToStaticMarkup(createElement(TopicWorkspace, {
      detail: { ...detail, mergedTopics: [{ ...topic, id: 'topic_old', name: 'old-parser-notes', title: 'Old parser notes', mergedIntoTopicId: topic.id }] },
      loading: false, error: null, saving: false, ...callbacks
    }));
    expect(html).toContain('Merged topics');
    expect(html).toContain('Old parser notes');
    expect(html).toContain('Undo merge');
    expect(html).not.toContain('Historical observation.');
  });

  it('creates a stable slug while retaining a fluent title', () => {
    expect(canonicalResearchTopicName('Parser / Review / Open Questions')).toBe('parser-review-open-questions');
    const source = readFileSync(new URL('../src/renderer/features/topics/TopicsExplorer.tsx', import.meta.url), 'utf8');
    expect(source).toContain('title: topicName.trim()');
  });
});
