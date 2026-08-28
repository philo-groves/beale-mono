import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { HoneycrispReportSummary, ProviderSettings, ResearchProviderModelCatalog, WorkspaceRegistryEntry } from '@shared/types';
import { INSET_SCROLLBAR_SELECTOR } from '../src/renderer/hooks/useInsetScrollbarActivation';
import { EditableReport, ReportSummarySidebar, ReportsIndex } from '../src/renderer/features/reports/ReportsWorkspace';
import {
  isReportResourceRun,
  reportChangeInstruction,
  reportMarkdownBlocks,
  replaceReportMarkdownBlock,
  reportSessionDefaultModelSelection,
  reportTitleFromMarkdown,
  reportsForReportingScope
} from '../src/renderer/view-models/reports';

const report: HoneycrispReportSummary = {
  id: 'report_parser',
  workspaceId: 'workspace_one',
  workspaceName: 'Parser',
  subjectId: 'subject_one',
  subjectName: 'Parser',
  sessionId: 'run_origin',
  title: 'Parser boundary confusion',
  summary: 'A verified parser boundary issue.',
  status: 'complete',
  triageStatus: 'editing',
  artifactId: 'artifact_report',
  submissionPacket: null,
  recording: null,
  revision: 3,
  revisions: [],
  authors: [
    { provider: 'openai', model: 'gpt-5.6' },
    { provider: 'zai', model: 'glm-5' }
  ],
  createdAt: '2026-08-10T12:00:00.000Z',
  updatedAt: '2026-08-16T12:00:00.000Z'
};

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
  updatedAt: '2026-08-16T12:00:00.000Z',
  lastOpenedAt: '2026-08-16T12:00:00.000Z',
  runCount: 1,
  lastRunAt: '2026-08-16T12:00:00.000Z'
};

describe('reports resource views', () => {
  it('uses the shared centered regular-weight loading state', () => {
    const html = renderToStaticMarkup(createElement(ReportsIndex, {
      reports: [],
      workspaces: [workspace],
      selectedWorkspaceId: null,
      loading: true,
      error: null,
      onScopeChange: () => undefined,
      onOpenReport: () => undefined
    }));

    expect(html).toContain('class="centered-loading-state"');
    expect(html).toContain('class="centered-loading-state-spinner"');
    expect(html).toContain('<span>Loading reports…</span>');
    expect(html).not.toContain('<strong>Loading reports');
  });

  it('gives report content the main surface and uses the default session sidenav width for its packet', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(styles).toMatch(/\.report-session-grid\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\) 6px 360px;/s);
    expect(styles).toMatch(/\.report-session-document\s*\{[^}]*background:\s*var\(--panel\);/s);
    expect(styles).toMatch(/\.report-session-document-scroll\s*\{[^}]*padding:\s*18px 24px 42px;/s);
    expect(styles).toMatch(/\.report-session-document::before,[\s\S]*?\.report-session-document::after\s*\{[^}]*height:\s*30px;[^}]*opacity:\s*0;/s);
    expect(styles).toContain('.report-session-document.has-top-fade::before');
    expect(styles).toContain('.report-session-document.has-bottom-fade::after');
    const reportSource = readFileSync(new URL('../src/renderer/features/reports/ReportsWorkspace.tsx', import.meta.url), 'utf8');
    expect(reportSource).toContain("frame.classList.toggle('has-top-fade'");
    expect(reportSource).toContain("frame.classList.toggle('has-bottom-fade'");
    expect(reportSource).toContain('onScroll={updateScrollEdges}');
    expect(styles).toMatch(/\.session-summary-item\.report-summary-item\s*\{[^}]*grid-template-columns:\s*18px minmax\(0, 1fr\) minmax\(0, 1fr\);/s);
    expect(INSET_SCROLLBAR_SELECTOR).toContain('.report-session-document-scroll');
    expect(styles).toMatch(/:where\([\s\S]*?\.main-commentary-list,[\s\S]*?\.report-session-document-scroll,[\s\S]*?\)\s*\{\s*scrollbar-color:\s*transparent transparent;/s);
    expect(styles).toMatch(/\.report-editable-block-content \.main-trace-markdown h1\s*\{[^}]*font-size:\s*1\.75rem;/s);
    expect(styles).toMatch(/\.report-editable-block-content \.main-trace-markdown h2\s*\{[^}]*font-size:\s*1\.45rem;/s);
    expect(styles).toMatch(/\.report-editable-block-content \.main-trace-markdown h3\s*\{[^}]*font-size:\s*1\.2rem;/s);
    expect(styles).toMatch(/\.report-editable-block-content \.main-trace-markdown h4\s*\{[^}]*font-size:\s*1\.05rem;/s);
    expect(styles).toMatch(/\.report-editable-block-content \.main-trace-markdown h5\s*\{[^}]*font-size:\s*0\.95rem;/s);
    expect(styles).toMatch(/\.report-editable-block-content \.main-trace-markdown h6\s*\{[^}]*font-size:\s*0\.85rem;/s);
  });

  it('moves report refinement into a locked report-editing quick chat', () => {
    const source = readFileSync(new URL('../src/renderer/features/reports/ReportsWorkspace.tsx', import.meta.url), 'utf8');
    const quickChatSource = readFileSync(new URL('../src/renderer/features/quick-chat/QuickChatDock.tsx', import.meta.url), 'utf8');
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(source).not.toContain('<CommentaryView');
    expect(appSource).toContain("kind: 'report-edit'");
    expect(appSource).toContain("kind: 'report-edit',\n      title,");
    expect(appSource).not.toContain('Report editing · ${title}');
    expect(appSource).toContain('}, ...quickChats]');
    expect(appSource).toContain('if (settingsOpen || !reportsOpen || !selectedReport) return quickChats;');
    expect(appSource).toContain('selectedReportDocument?.content, settingsOpen, snapshot?.runs]);');
    expect(quickChatSource).toContain("chat.kind === 'report-edit'");
    expect(quickChatSource).toContain("'Describe a change to this report'");
    expect(quickChatSource).toContain('!reportEditing ? (');
    expect(styles).toContain('.quick-chat-card.is-report-edit .quick-chat-header');
    expect(styles).toMatch(/\.quick-chat-card\.is-report-edit\s*\{[^}]*height:\s*clamp\(260px, calc\(100vh - 250px\), 570px\);/s);
  });

  it('opens reports without starting an agent and offers a report-specific quick-chat suggestion', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const reportSource = readFileSync(new URL('../src/renderer/features/reports/ReportsWorkspace.tsx', import.meta.url), 'utf8');
    const openHandler = appSource.slice(
      appSource.indexOf('const openReportSession'),
      appSource.indexOf('const startReportTurn')
    );
    expect(openHandler).not.toContain('startReportSession');
    const quickChatSource = readFileSync(new URL('../src/renderer/features/quick-chat/QuickChatDock.tsx', import.meta.url), 'utf8');
    expect(reportSource).not.toContain('<CommentaryView');
    expect(quickChatSource).toContain("'Review and improve this report.'");
  });

  it('uses the default provider large model instead of the first catalog model', () => {
    const settings: ProviderSettings = {
      defaultProviderId: 'openai-codex',
      modelDefaults: {
        'openai-codex': {
          largeModel: 'gpt-5.6-sol',
          smallModel: 'gpt-5.4-mini',
          reasoningEffort: 'xhigh'
        }
      }
    };
    const catalogs: ResearchProviderModelCatalog[] = [{
      providerId: 'openai-codex',
      providerName: 'OpenAI',
      models: [
        { id: 'gpt-5.3-codex-spark', name: 'Codex Spark', reasoning: true, effortLevels: ['low', 'high'], contextWindow: 128_000, maxTokens: 16_000 },
        { id: 'gpt-5.6-sol', name: 'GPT-5.6', reasoning: true, effortLevels: ['high', 'xhigh'], contextWindow: 256_000, maxTokens: 32_000 }
      ]
    }];

    expect(reportSessionDefaultModelSelection(settings, catalogs)).toEqual({
      provider: 'openai-codex',
      model: 'gpt-5.6-sol',
      reasoningEffort: 'xhigh'
    });
  });

  it('uses the provider account default when the Lead provider has no stored model defaults', () => {
    const settings: ProviderSettings = {
      defaultProviderId: 'openai-codex',
      modelDefaults: {}
    };
    const catalogs: ResearchProviderModelCatalog[] = [{
      providerId: 'openai-codex',
      providerName: 'OpenAI',
      models: [
        { id: 'gpt-5.3-codex-spark', name: 'Codex Spark', reasoning: true, effortLevels: ['low', 'high'], contextWindow: 128_000, maxTokens: 16_000 },
        { id: 'gpt-5.6-sol', name: 'GPT-5.6', reasoning: true, effortLevels: ['high', 'xhigh'], contextWindow: 256_000, maxTokens: 32_000 }
      ]
    }];

    expect(reportSessionDefaultModelSelection(settings, catalogs, {
      defaultModel: 'gpt-5.6-sol',
      defaultReasoningEffort: 'xhigh'
    })).toEqual({
      provider: 'openai-codex',
      model: 'gpt-5.6-sol',
      reasoningEffort: 'xhigh'
    });
  });

  it('does not silently replace an unavailable preferred model with the first catalog model', () => {
    expect(reportSessionDefaultModelSelection({
      defaultProviderId: 'openai-codex',
      modelDefaults: {}
    }, [{
      providerId: 'openai-codex',
      providerName: 'OpenAI',
      models: [
        { id: 'gpt-5.3-codex-spark', name: 'Codex Spark', reasoning: true, effortLevels: ['low', 'high'], contextWindow: 128_000, maxTokens: 16_000 }
      ]
    }], {
      defaultModel: 'gpt-5.6-sol',
      defaultReasoningEffort: 'xhigh'
    })).toBeNull();
  });

  it('loads provider settings and the model catalog for an idle report view', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    expect(appSource).toContain("if (!newResearchOpen && !automationsOpen && !reportsOpen && quickChats.length === 0 && !(settingsOpen && settingsSection === 'providers')) return;");
    expect(appSource).toContain("if (!openWorkspaceId && !newResearchOpen && !automationsOpen && !reportsOpen && quickChats.length === 0 && !selectedRunId && !(settingsOpen && settingsSection === 'providers')) return;");
  });

  it('enables the safety selector before the first report message and applies it when starting the run', () => {
    const composerSource = readFileSync(new URL('../src/renderer/features/sessions/SessionComposer.tsx', import.meta.url), 'utf8');
    const serviceSource = readFileSync(new URL('../src/main/workspaceService.ts', import.meta.url), 'utf8');
    expect(composerSource).toContain("disabled={busy || status === 'paused' || (!runId && !onInitialInstruction)}");
    expect(composerSource).toContain('onInitialInstruction?.(trimmedInstruction, modelSelection, shellSafetyMode)');
    expect(serviceSource).toContain('shellSafetyMode: normalizeShellSafetyMode(input.shellSafetyMode)');
  });

  it('persists the report identity so Honeycrisp can resolve its canonical model context', () => {
    const serviceSource = readFileSync(new URL('../src/main/workspaceService.ts', import.meta.url), 'utf8');
    expect(serviceSource).toContain("kind: 'report'");
    expect(serviceSource).toContain('resourceId: report.id');
    expect(serviceSource).toContain('artifactId: report.artifactId');
    expect(serviceSource).toContain('artifactRelativePath: artifact.relativePath');
    expect(serviceSource).toContain('revision: report.revision');
    expect(serviceSource).toContain('promptMarkdown: instruction');
    expect(serviceSource).not.toContain('Open the existing workspace report');
  });

  it('keeps report editor runs out of ordinary workspace session surfaces', () => {
    expect(isReportResourceRun({ budget: { resourceContext: { kind: 'report', resourceId: report.id } } })).toBe(true);
    expect(isReportResourceRun({ budget: { maxMinutes: 10 } })).toBe(false);
  });

  it('places Reporting directly below Automations in the workspace sidenav', () => {
    const source = readFileSync(new URL('../src/renderer/features/workspaces/WorkspaceSidebar.tsx', import.meta.url), 'utf8');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(source.indexOf('<span>Reporting</span>')).toBeGreaterThan(-1);
    expect(source.indexOf('<span>Reporting</span>')).toBeGreaterThan(source.indexOf('<span>Automations</span>'));
    expect(source.indexOf('<span>Reporting</span>')).toBeLessThan(source.indexOf('<span>Plugins</span>'));
    expect(styles).toMatch(/\.sidebar-utility-button\s*\{[^}]*background:\s*transparent;/s);
    expect(styles).toMatch(/\.sidebar-utility-button:hover:not\(:disabled\)\s*\{[^}]*background:\s*color-mix\(in srgb, var\(--text\) 4\.5%, transparent\);/s);
    expect(styles).toMatch(/\.sidebar-utility-button\.active\s*\{[^}]*background:\s*var\(--panel\);/s);
    expect(styles).toMatch(/\.sidebar-utility-button\.active:hover:not\(:disabled\)\s*\{[^}]*background:\s*var\(--panel\);/s);
  });

  it('renders the workspace report catalog and current report state', () => {
    const staleReport = {
      ...report,
      id: 'report_stale',
      title: 'Parser follow-up',
      status: 'stale' as const,
      triageStatus: 'accepted' as const
    };
    const html = renderToStaticMarkup(createElement(ReportsIndex, {
      reports: [report, staleReport],
      workspaces: [workspace],
      selectedWorkspaceId: null,
      loading: false,
      error: null,
      onScopeChange: () => undefined,
      onOpenReport: () => undefined
    }));

    expect(html).toContain('All Reports');
    expect(html).toContain('role="tablist"');
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain('>Parser</span>');
    expect(html).toContain('Parser boundary confusion');
    expect(html).toContain('Parser follow-up');
    expect(html).toContain('<small>Editing</small>');
    expect(html).toContain('<small>Accepted</small>');
    expect(html.match(/<span>Edit<\/span>/g)).toHaveLength(2);
    expect(html).not.toContain('A verified parser boundary issue.');
    expect(html).not.toContain('Update 3');
    expect(html).not.toContain('<h2>1 Complete</h2>');
    expect(html).not.toContain('<h2>1 Stale</h2>');
    expect(html).not.toContain('reports-index-eyebrow');
    expect(html).toContain('<h1>All Reporting</h1>');
    expect(html).toContain('Review, edit, and prepare reports created during research sessions.');
    expect(html.indexOf('role="tablist"')).toBeLessThan(html.indexOf('<h1>All Reporting</h1>'));
  });

  it('matches Profile settings content spacing and symmetric tab padding', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');

    expect(styles).toMatch(/\.reports-index\s*\{[^}]*width:\s*100%;[^}]*padding:\s*10px;/s);
    expect(styles).toMatch(/\.reports-index-tabs,[\s\S]*?\.reports-index-empty\s*\{[^}]*width:\s*100%;/s);
    expect(styles).toMatch(/\.reports-index-tab\.provider-settings-tab \.research-side-view-tab-activate\s*\{[^}]*padding:\s*0 9px;/s);
    expect(styles).toMatch(/\.resource-workspace-heading\s*\{[^}]*max-width:\s*var\(--session-content-max-width\);[^}]*margin-inline:\s*auto;/s);
    expect(styles).toMatch(/\.resource-workspace-heading h1\s*\{[^}]*font-size:\s*26px;[^}]*font-weight:\s*400;/s);
    expect(styles).toMatch(/\.settings-workspace\s*\{[^}]*background:\s*var\(--panel\);/s);
    expect(styles).toMatch(/\.settings-main-view\s*\{[^}]*padding:\s*10px;/s);
    expect(styles).toMatch(/\.profile-settings-tab \.research-side-view-tab-activate\s*\{[^}]*padding:\s*0 9px;/s);
  });

  it('uses an Archived Sessions-style flat list with edit actions', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');

    expect(styles).toMatch(/\.reports-index-list\s*\{[^}]*border-radius:\s*26px;[^}]*background:\s*var\(--panel-raised\);[^}]*padding:\s*3px 14px;/s);
    expect(styles).toMatch(/\.reports-index-list\s*\{[^}]*max-width:\s*var\(--session-content-max-width\);[^}]*margin-inline:\s*auto;/s);
    expect(styles).toMatch(/\.reports-index-row\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\) auto;[^}]*padding:\s*10px 0;/s);
    expect(styles).toMatch(/\.reports-index-row \+ \.reports-index-row\s*\{[^}]*border-top:\s*1px solid var\(--line\);/s);
    expect(styles).toMatch(/\.reports-index-edit-button\s*\{[^}]*background:\s*var\(--panel-strong\);/s);
  });

  it('renders an explicit empty state before an agent creates a report', () => {
    const html = renderToStaticMarkup(createElement(ReportsIndex, {
      reports: [],
      workspaces: [workspace],
      selectedWorkspaceId: workspace.workspaceId,
      loading: false,
      error: null,
      onScopeChange: () => undefined,
      onOpenReport: () => undefined
    }));

    expect(html).toContain('No reports yet');
    expect(html).toContain('Reports created by agents during research sessions');
    expect(html).toContain('<h1>Parser Reporting</h1>');
  });

  it('defaults Reporting to the selected workspace and otherwise supports all-workspace scope', () => {
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const sidebarSource = readFileSync(new URL('../src/renderer/features/workspaces/WorkspaceSidebar.tsx', import.meta.url), 'utf8');
    const otherReport = { ...report, id: 'report_other', workspaceId: 'workspace_two', workspaceName: 'Other' };

    expect(reportsForReportingScope([report, otherReport], null)).toHaveLength(2);
    expect(reportsForReportingScope([report, otherReport], workspace.workspaceId)).toEqual([report]);
    expect(appSource).toContain('setReportingScopeWorkspaceId(snapshot?.workspace.workspaceId ?? null);');
    expect(appSource).toContain('reportsOpen ? (');
    expect(sidebarSource).not.toMatch(/title="Reporting"[^>]*disabled=\{!snapshot\}/);
  });

  it('loads reporting data and report documents through workspace-independent host contracts', () => {
    const apiSource = readFileSync(new URL('../src/shared/types.ts', import.meta.url), 'utf8');
    const serviceSource = readFileSync(new URL('../src/main/workspaceService.ts', import.meta.url), 'utf8');
    const mainSource = readFileSync(new URL('../src/main/index.ts', import.meta.url), 'utf8');

    expect(apiSource).toContain('listReportingReports(): Promise<HoneycrispReportSummary[]>;');
    expect(apiSource).toContain('getHoneycrispReport(locator: HoneycrispReportLocator)');
    expect(apiSource).toContain('updateReportContent(input: ReportContentUpdateInput)');
    expect(apiSource).toContain('updateReportTriageStatus(input: ReportTriageStatusUpdateInput)');
    expect(apiSource).toContain('openReportSubmissionPacket(locator: HoneycrispReportLocator)');
    expect(apiSource).toContain('chooseReportSubmissionPacket(locator: HoneycrispReportLocator)');
    expect(apiSource).toContain('chooseReportRecording(locator: HoneycrispReportLocator)');
    expect(serviceSource).toContain('public async listReportingReports(): Promise<HoneycrispReportSummary[]>');
    expect(serviceSource).toContain('workspaceId: workspace.workspaceId');
    expect(serviceSource).toContain('Report workspace is not registered');
    expect(serviceSource).toContain("resolveHoneycrispArtifact(report.submissionPacket.artifactId");
    expect(serviceSource).toContain("'submission-packet'");
    expect(mainSource).toContain('IPC_CHANNELS.openReportSubmissionPacket');
    expect(mainSource).toContain('IPC_CHANNELS.updateReportContent');
    expect(mainSource).toContain('IPC_CHANNELS.updateReportTriageStatus');
    expect(mainSource).toContain('IPC_CHANNELS.chooseReportSubmissionPacket');
    expect(mainSource).toContain('IPC_CHANNELS.chooseReportRecording');
    expect(mainSource).toContain("filters: [{ name: 'ZIP archives', extensions: ['zip'] }]");
    expect(mainSource).toContain("name: 'Media files'");
    expect(serviceSource).toContain('public async replaceReportSubmissionPacket(');
    expect(serviceSource).toContain('public async updateReportContent(');
    expect(serviceSource).toContain('reviseHoneycrispReportContent({');
    expect(serviceSource).toContain('public async updateReportTriageStatus(');
    expect(serviceSource).toContain('updateHoneycrispReportTriageStatus({');
    expect(serviceSource).toContain('replaceHoneycrispReportSubmissionPacket({');
    expect(serviceSource).toContain('public async replaceReportRecording(');
    expect(serviceSource).toContain('replaceHoneycrispReportRecording({');
    expect(mainSource).toContain('workspaceService.resolveReportSubmissionPacketPath(locator)');
  });

  it('makes report blocks targetable for inline change requests', () => {
    const html = renderToStaticMarkup(createElement(EditableReport, {
      report,
      document: { reportId: report.id, content: '# Summary\n\nVerified impact.\n\n## Evidence\n\n- verifier:pass' },
      loading: false,
      error: null,
      onChange: async () => undefined,
      onMarkdownChange: async () => undefined
    }));

    expect(html).not.toContain('report-session-document-header');
    expect(html).not.toContain('Authored by');
    expect(html).not.toContain('class="report-session-back"');
    expect(html).not.toContain('>Report</span>');
    expect(html).toContain('Editable report content');
    expect(html).toContain('Edit report lines 1 through 1');
    expect(html).not.toContain('Shift-click');
  });

  it('shows an existing packet filename as the replacement file-picker action', () => {
    const packetReport: HoneycrispReportSummary = {
      ...report,
      submissionPacket: {
        artifactId: 'report_parser_submission_packet',
        filename: 'submission.zip',
        sizeBytes: 12_288,
        contentHash: 'sha256:0123456789abcdef'
      },
      recording: {
        artifactId: 'report_parser_recording',
        filename: 'parser-demo.mov',
        sizeBytes: 32_768,
        contentHash: 'sha256:abcdef0123456789'
      }
    };
    const html = renderToStaticMarkup(createElement(ReportSummarySidebar, {
      report: packetReport,
      onStatusChange: async () => undefined,
      onChooseSubmissionPacket: async () => undefined,
      onChooseRecording: async () => undefined
    }));

    expect(html).toContain('<span>Packet</span>');
    expect(html).toContain('submission.zip');
    expect(html).toContain('<span>Recording</span>');
    expect(html).toContain('parser-demo.mov');
    expect(html).not.toContain('12.0 KB');
    expect(html).not.toContain('sha256:0123456789abcdef');
    expect(html).not.toContain('Open packet');
  });

  it('renders packet and recording attachments in a session-style report summary sidenav', () => {
    const html = renderToStaticMarkup(createElement(ReportSummarySidebar, {
      report,
      onStatusChange: async () => undefined,
      onChooseSubmissionPacket: async () => undefined,
      onChooseRecording: async () => undefined
    }));

    expect(html).toContain('aria-label="Report summary"');
    expect(html).toContain('class="session-summary-card"');
    expect(html).toContain('<h2 class="session-summary-title">Report</h2>');
    expect(html).toContain('<span>Packet</span>');
    expect(html).toContain('<span>Status</span>');
    expect(html).toContain('aria-label="Report status"');
    expect(html).toContain('<option value="editing" selected="">Editing</option>');
    expect(html).toContain('<option value="submitted">Submitted</option>');
    expect(html).toContain('<option value="reviewing">Reviewing</option>');
    expect(html).toContain('<option value="rejected">Rejected</option>');
    expect(html).toContain('<option value="accepted">Accepted</option>');
    expect(html.indexOf('<span>Status</span>')).toBeLessThan(html.indexOf('<span>Packet</span>'));
    expect(html).toContain('<span>Recording</span>');
    expect(html).toContain('Choose File');
    expect(html).not.toContain(report.title);
    expect(html).not.toContain(report.summary);
    expect(html).not.toContain('Authored by');
    expect(html).not.toContain('<dl');
    expect(html).not.toContain('report-ticketing-action');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(styles).toMatch(/\.report-summary-status select\s*\{[^}]*justify-self:\s*end;[^}]*background-color:\s*transparent;/s);
  });

  it('uses the first Markdown line as the report title without Markdown symbols', () => {
    expect(reportTitleFromMarkdown('# **Parser** `boundary` confusion', report.title)).toBe('Parser boundary confusion');
    expect(reportTitleFromMarkdown('[Parser boundary](https://example.test) confusion', report.title)).toBe('Parser boundary confusion');
    expect(reportTitleFromMarkdown('', report.title)).toBe(report.title);
  });

  it('replaces one Markdown block without disturbing surrounding report content', () => {
    const content = '# Summary\n\nOriginal impact.\n\n## Evidence';
    const block = reportMarkdownBlocks(content)[1]!;
    expect(replaceReportMarkdownBlock(content, block, 'Updated **impact**.'))
      .toBe('# Summary\n\nUpdated **impact**.\n\n## Evidence');
  });

  it('opens one raw Markdown editor with a compact agent edit-request composer', () => {
    const source = readFileSync(new URL('../src/renderer/features/reports/ReportsWorkspace.tsx', import.meta.url), 'utf8');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(source).toContain('className="report-markdown-editor"');
    expect(source).toContain('style={{ height: markdownEditorHeight }}');
    expect(source).toContain('placeholder="Request an edit"');
    expect(source).toContain('className="main-steer-send report-edit-request-send"');
    expect(source).not.toContain('report-inline-edit-scope');
    expect(source).not.toContain('report-session-edit-hint');
    expect(source).not.toContain('selectedBlockIds');
    expect(styles).toMatch(/\.report-edit-request\s*\{[^}]*width:\s*min\(520px, calc\(100% - 28px\)\);[^}]*position:\s*absolute;[^}]*top:\s*100%;[^}]*left:\s*14px;[^}]*corner-shape:\s*squircle;[^}]*box-shadow:/s);
    expect(styles).toMatch(/\.report-editable-block\.is-editing\s*\{[^}]*z-index:\s*4;/s);
    expect(styles).toMatch(/\.report-edit-request\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\) 35px;/s);
    expect(styles).toMatch(/\.report-edit-request input:focus,[\s\S]*?\.report-edit-request input:focus-visible\s*\{[^}]*outline:\s*none;[^}]*box-shadow:\s*none;/s);
    expect(styles).toMatch(/\.report-markdown-editor\s*\{[^}]*box-sizing:\s*border-box;[^}]*resize:\s*vertical;/s);
  });

  it('keeps fenced markdown together and scopes edit requests to one block', () => {
    const blocks = reportMarkdownBlocks('## Proof\n\n```python\nprint("ok")\n\nprint("still fenced")\n```\n\nImpact');
    expect(blocks).toHaveLength(3);
    expect(blocks[1]?.content).toContain('still fenced');

    const instruction = reportChangeInstruction(blocks[2]!, 'Clarify the affected versions.');
    expect(instruction).not.toContain(report.id);
    expect(instruction).not.toContain(report.title);
    expect(instruction).not.toContain('report.get');
    expect(instruction).toContain('Selected report block: lines 9-9.');
    expect(instruction).toContain('only this report block');
    expect(instruction).toContain('Clarify the affected versions.');
  });
});
