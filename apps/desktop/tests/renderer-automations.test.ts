import { existsSync, readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { AutomationSummary, RunDetail, WorkspaceRegistryEntry } from '@shared/types';
import {
  automationAttemptDetail,
  AutomationsWorkspace,
  orderedAutomationAttempts
} from '../src/renderer/features/automations/AutomationsWorkspace';
import { researchSettingsInput } from '../src/renderer/features/sessions/StartRunForm';
import { defaultRunInput } from '../src/renderer/view-models/runSettings';

const workspace: WorkspaceRegistryEntry = {
  id: 'registry_workspace_one',
  workspacePath: 'C:\\workspaces\\parser',
  workspaceId: 'workspace_one',
  workspaceName: 'Parser',
  researchProfileId: 'security-research',
  researchKitId: 'general',
  scopeOwner: 'Parser',
  descriptionMarkdown: '',
  rulesMarkdown: '',
  expiresAt: null,
  createdAt: '2026-08-10T12:00:00.000Z',
  updatedAt: '2026-08-17T12:00:00.000Z',
  lastOpenedAt: '2026-08-17T12:00:00.000Z',
  runCount: 2,
  lastRunAt: '2026-08-17T12:00:00.000Z'
};

const activeAutomation: AutomationSummary = {
  runId: 'run_active',
  workspaceId: workspace.workspaceId,
  workspaceName: workspace.workspaceName,
  title: 'Daily parser review',
  promptPreview: 'Review parser boundary changes.',
  enabled: true,
  schedule: { type: 'daily', interval: 1 },
  maxMinutes: 45,
  maxAttempts: 3,
  maxCostUsd: 8,
  settings: {
    ...defaultRunInput,
    provider: 'openai-codex',
    promptMarkdown: 'Review parser boundary changes.',
    workflowId: 'discovery',
    model: 'gpt-5.6-sol',
    budget: {
      ...defaultRunInput.budget,
      maxMinutes: 45,
      maxAttempts: 3,
      maxCostUsd: 8,
      repeatSchedule: { type: 'daily', interval: 1 }
    }
  },
  researchProfile: null,
  sessionStatus: 'completed',
  createdAt: '2026-08-16T12:00:00.000Z',
  updatedAt: '2026-08-17T12:00:00.000Z'
};

const inactiveAutomation: AutomationSummary = {
  ...activeAutomation,
  runId: 'run_inactive',
  title: 'Weekly regression review',
  enabled: false,
  schedule: { type: 'weekly', interval: 1 },
  settings: {
    ...activeAutomation.settings,
    budget: {
      ...activeAutomation.settings.budget,
      repeatSchedule: { type: 'weekly', interval: 1 }
    }
  }
};

const automationDetail = {
  run: {
    id: activeAutomation.runId,
    scopeVersionId: 'scope_one',
    researchProfileSnapshotId: null,
    shellSafetyMode: activeAutomation.settings.shellSafetyMode,
    mode: activeAutomation.settings.mode,
    status: 'completed',
    title: activeAutomation.title,
    promptMarkdown: activeAutomation.settings.promptMarkdown,
    model: activeAutomation.settings.model,
    reasoningEffort: activeAutomation.settings.reasoningEffort,
    attemptStrategy: activeAutomation.settings.attemptStrategy,
    sandboxProfile: activeAutomation.settings.sandboxProfile,
    targetAssetId: null,
    targetPath: null,
    budget: { ...activeAutomation.settings.budget, modelProvider: activeAutomation.settings.provider },
    summary: '',
    finalDisposition: null,
    createdAt: '2026-08-16T12:00:00.000Z',
    startedAt: '2026-08-16T12:00:00.000Z',
    endedAt: '2026-08-18T12:10:00.000Z'
  },
  attempts: [{
    id: 'attempt_old',
    runId: activeAutomation.runId,
    parentAttemptId: null,
    status: 'completed',
    shortState: 'done',
    seed: 'old',
    strategyRole: 'primary',
    cost: {},
    tokenUsage: {},
    startedAt: '2026-08-17T12:00:00.000Z',
    endedAt: '2026-08-17T12:10:00.000Z'
  }, {
    id: 'attempt_latest',
    runId: activeAutomation.runId,
    parentAttemptId: 'attempt_old',
    status: 'failed',
    shortState: 'failed',
    seed: 'latest',
    strategyRole: 'primary',
    cost: {},
    tokenUsage: {},
    startedAt: '2026-08-18T12:00:00.000Z',
    endedAt: '2026-08-18T12:10:00.000Z'
  }],
  traceEvents: [{
    id: 'trace_old',
    runId: activeAutomation.runId,
    attemptId: 'attempt_old',
    sequence: 1,
    type: 'research_event',
    source: 'model',
    summary: 'Old run commentary',
    payload: {},
    sensitivity: 'internal',
    modelVisible: true,
    createdAt: '2026-08-17T12:01:00.000Z',
    artifactId: null,
    toolCallId: null,
    approvalId: null
  }, {
    id: 'trace_latest',
    runId: activeAutomation.runId,
    attemptId: 'attempt_latest',
    sequence: 2,
    type: 'research_event',
    source: 'model',
    summary: 'Latest run commentary',
    payload: {},
    sensitivity: 'internal',
    modelVisible: true,
    createdAt: '2026-08-18T12:01:00.000Z',
    artifactId: null,
    toolCallId: null,
    approvalId: null
  }],
  transcriptMessages: [{
    id: 'message_old',
    runId: activeAutomation.runId,
    attemptId: 'attempt_old',
    traceEventId: null,
    role: 'assistant',
    phase: 'final_answer',
    contentMarkdown: 'Old run commentary',
    source: 'model',
    metadata: {},
    createdAt: '2026-08-17T12:02:00.000Z'
  }, {
    id: 'message_latest',
    runId: activeAutomation.runId,
    attemptId: 'attempt_latest',
    traceEventId: null,
    role: 'assistant',
    phase: 'final_answer',
    contentMarkdown: 'Latest run commentary',
    source: 'model',
    metadata: {},
    createdAt: '2026-08-18T12:02:00.000Z'
  }],
  artifacts: [],
  verifierContracts: [],
  verifierRuns: [],
  modelSessions: [],
  contextCompactions: [],
  policyEvents: [],
  exports: []
} as RunDetail;

function render(
  selectedAutomation: AutomationSummary | null = null,
  loading = false,
  selectedWorkspaceId: string | null = null
): string {
  return renderToStaticMarkup(createElement(AutomationsWorkspace, {
    automations: [activeAutomation, inactiveAutomation],
    workspaces: [workspace],
    selectedWorkspaceId,
    selectedAutomation,
    detail: selectedAutomation ? automationDetail : null,
    sessionSetupPending: false,
    activeScope: null,
    providerModelDefaults: {},
    providerModelCatalog: [{
      providerId: 'openai-codex',
      providerName: 'OpenAI',
      models: [{
        id: 'gpt-5.6-sol',
        name: 'GPT-5.6 Sol',
        reasoning: true,
        effortLevels: ['low', 'medium', 'high', 'xhigh'],
        contextWindow: 400_000,
        maxTokens: 128_000
      }]
    }],
    shellApproval: null,
    shellApprovalBusy: false,
    dangerModeEnabled: false,
    responseSuggestionsEnabled: true,
    busy: false,
    loading,
    error: null,
    onScopeChange: () => undefined,
    onSelectAutomation: () => undefined,
    onShellApprovalDecision: () => undefined,
    onSessionAction: () => undefined,
    onSteerInstruction: () => undefined
  }));
}

describe('automation workspace', () => {
  it('uses the shared centered regular-weight loading state', () => {
    const html = render(null, true);

    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('class="centered-loading-state"');
    expect(html).toContain('class="centered-loading-state-spinner"');
    expect(html).toContain('<span>Loading automations…</span>');
    expect(html).not.toContain('<strong>Loading automations');
  });

  it('keeps New Research defaults uninflated and preserves stored automation choices when inflated', () => {
    const defaults = researchSettingsInput(undefined, 'discovery', null);
    const dangerDefaults = researchSettingsInput(undefined, 'discovery', null, 'danger');
    const inflated = researchSettingsInput(activeAutomation.settings, 'longshot', null);

    expect(defaults.promptMarkdown).toBe('');
    expect(defaults.model).toBe('');
    expect(defaults.shellSafetyMode).toBe('auto_review');
    expect(dangerDefaults.shellSafetyMode).toBe('danger');
    expect(defaults.budget.repeatSchedule).toEqual({ type: 'none' });
    expect(inflated).toMatchObject({
      provider: 'openai-codex',
      workflowId: 'discovery',
      model: 'gpt-5.6-sol',
      promptMarkdown: 'Review parser boundary changes.',
      budget: { repeatSchedule: { type: 'daily', interval: 1 } }
    });
  });

  it('renders All Automations and workspace scope tabs with a flat status list', () => {
    const html = render();

    expect(html).toContain('All Automations');
    expect(html).toContain('<h1>All Automations</h1>');
    expect(html).toContain('Manage scheduled research sessions across your workspaces.');
    expect(html.indexOf('role="tablist"')).toBeLessThan(html.indexOf('<h1>All Automations</h1>'));
    expect(render(null, false, workspace.workspaceId)).toContain('<h1>Parser Automations</h1>');
    expect(html).toContain('role="tablist"');
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain('>Parser</span>');
    expect(html).toContain('Daily parser review');
    expect(html).toContain('Weekly regression review');
    expect(html).toContain('<small>Active</small>');
    expect(html).toContain('<small>Inactive</small>');
    expect(html.match(/<span>Edit<\/span>/g)).toHaveLength(2);
    expect(html).not.toContain('<h2>1 Active</h2>');
    expect(html).not.toContain('<h2>1 Inactive</h2>');
  });

  it('opens the latest run commentary with a prefilled standard session composer and run switcher', () => {
    const html = render(activeAutomation);

    expect(html).toContain('class="report-session-grid automation-session-grid"');
    expect(html).toContain('aria-label="Automation"');
    expect(html).toContain('<h2 class="session-summary-title">Automation</h2>');
    expect(html).toContain('<span>Interval</span>');
    expect(html).toContain('<span class="session-summary-meta">Daily</span>');
    expect(html.indexOf('<span>Interval</span>')).toBeLessThan(html.indexOf('class="session-summary-divider automation-run-divider"'));
    expect(html.indexOf('class="session-summary-divider automation-run-divider"')).toBeLessThan(html.indexOf('2026-08-18T12:00:00.000Z'));
    expect(html.indexOf('2026-08-18T12:00:00.000Z')).toBeLessThan(html.indexOf('2026-08-17T12:00:00.000Z'));
    expect(html).toContain('Latest run commentary');
    expect(html).not.toContain('Old run commentary');
    expect(html).toContain('class="main-steer-input-row without-trace-filters"');
    expect(html).toContain('Review parser boundary changes.');
    expect(html).toContain('aria-label="Shell safety mode"');
    expect(html).toContain('aria-label="Collaboration settings"');
    expect(html).toContain('aria-label="Model settings for the next agent turn"');
    expect(html).toContain('<span class="model-selection-picker-model">5.6 Sol</span>');
  });

  it('orders automation attempts newest first and isolates commentary to the selected attempt', () => {
    expect(orderedAutomationAttempts(automationDetail.attempts).map((attempt) => attempt.id))
      .toEqual(['attempt_latest', 'attempt_old']);
    const selected = automationAttemptDetail(automationDetail, 'attempt_old');
    expect(selected?.traceEvents.map((event) => event.id)).toEqual(['trace_old']);
    expect(selected?.transcriptMessages.map((message) => message.id)).toEqual(['message_old']);
  });

  it('matches the centered Reporting list and removes dashed outlines', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');

    expect(styles).toMatch(/\.automations-workspace-list\s*\{[^}]*max-width:\s*var\(--session-content-max-width\);[^}]*margin-inline:\s*auto;[^}]*border-radius:\s*26px;[^}]*background:\s*var\(--panel-raised\);/s);
    expect(styles).toMatch(/\.resource-workspace-heading\s*\{[^}]*max-width:\s*var\(--session-content-max-width\);[^}]*margin-inline:\s*auto;/s);
    expect(styles).toMatch(/\.automation-row\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\) auto;[^}]*padding:\s*10px 0;/s);
    expect(styles).toMatch(/\.automation-row \+ \.automation-row\s*\{[^}]*border-top:\s*1px solid var\(--line\);/s);
    expect(styles).toMatch(/\.automation-edit-button\s*\{[^}]*background:\s*var\(--panel-strong\);/s);
    expect(styles).toMatch(/\.automation-interval-row\s*\{[^}]*grid-template-columns:\s*18px minmax\(0, 1fr\) auto;[^}]*cursor:\s*default;/s);
    expect(styles).toMatch(/\.automations-workspace-empty\s*\{[^}]*border:\s*0;/s);
    expect(styles).not.toContain('.automation-card');
  });

  it('moves Automations out of AppModals and into main navigation', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const modalSource = readFileSync(new URL('../src/renderer/app/AppModals.tsx', import.meta.url), 'utf8');
    const sidebarSource = readFileSync(new URL('../src/renderer/features/workspaces/WorkspaceSidebar.tsx', import.meta.url), 'utf8');

    expect(appSource).toContain('<AutomationsWorkspace');
    expect(appSource).toContain('!newResearchOpen && !automationsOpen && !reportsOpen');
    expect(appSource).toContain('!newResearchOpen && !automationsOpen && quickChats.length === 0 && !(settingsOpen');
    expect(appSource).toContain('setAutomationScopeWorkspaceId(snapshot?.workspace.workspaceId ?? null);');
    expect(appSource).toContain("? { primary: 'Automations', secondary: selectedAutomation?.title ?? automationScopeName, icon: 'automations' }");
    expect(modalSource).not.toContain('AutomationsModal');
    expect(sidebarSource).toContain("sidebar-utility-button${automationsActive && !workspaceCreationActive ? ' active' : ''}");
    expect(existsSync(new URL('../src/renderer/features/plugins/AutomationsModal.tsx', import.meta.url))).toBe(false);
  });

  it('exposes workspace-independent list and update contracts with retained inactive schedules', () => {
    const types = readFileSync(new URL('../src/shared/types.ts', import.meta.url), 'utf8');
    const service = readFileSync(new URL('../src/main/workspaceService.ts', import.meta.url), 'utf8');
    const database = readFileSync(new URL('../../../app-server/src/workspaceDatabase.ts', import.meta.url), 'utf8');

    expect(types).toContain('listAutomations(): Promise<AutomationSummary[]>;');
    expect(types).toContain('updateAutomation(input: AutomationUpdateInput): Promise<AutomationSummary>;');
    expect(service).toContain('public async listAutomations(): Promise<AutomationSummary[]>');
    expect(service).toContain('public updateAutomation(input: AutomationUpdateInput): AutomationSummary');
    expect(service).toContain("repeatSchedule: input.enabled ? schedule : { type: 'none' }");
    expect(service).toContain('automationSchedule: schedule');
    expect(service).toContain('runtime.db.updateRunModelSelection');
    expect(service).toContain('runtime.db.updateRunPrompt');
    expect(database).toContain('nextBudget.automationSchedule = schedule;');
  });
});
