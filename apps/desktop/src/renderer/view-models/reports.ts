import type {
  AppServerReportSummary,
  OpenAiAccountStatus,
  ProviderSettings,
  ResearchModelEffortLevel,
  ResearchModelSelection,
  ResearchProviderModelCatalog,
  ResearchProviderStatus,
  RunRecord
} from '@shared/types';

export function reportSessionDefaultModelSelection(
  providerSettings: ProviderSettings | null,
  catalogs: readonly ResearchProviderModelCatalog[],
  openAiStatus: Pick<OpenAiAccountStatus, 'defaultModel' | 'defaultReasoningEffort'> | null = null,
  providerStatuses: readonly Pick<ResearchProviderStatus, 'id' | 'defaultModel'>[] = []
): ResearchModelSelection | null {
  const provider = catalogs.find((catalog) => (
    catalog.providerId === providerSettings?.defaultProviderId && catalog.models.length > 0
  )) ?? catalogs.find((catalog) => catalog.models.length > 0);
  if (!provider) return null;
  const defaults = providerSettings?.modelDefaults[provider.providerId];
  const providerDefaultModel = provider.providerId === 'openai-codex'
    ? openAiStatus?.defaultModel
    : providerStatuses.find((status) => status.id === provider.providerId)?.defaultModel;
  const preferredModel = defaults?.largeModel ?? providerDefaultModel;
  const model = preferredModel
    ? provider.models.find((candidate) => candidate.id === preferredModel) ?? null
    : provider.models[0];
  if (!model) return null;
  const preferredEffort = defaults?.reasoningEffort
    ?? (provider.providerId === 'openai-codex' ? researchModelEffort(openAiStatus?.defaultReasoningEffort) : null)
    ?? 'high';
  const reasoningEffort: ResearchModelEffortLevel = model.effortLevels.includes(preferredEffort)
    ? preferredEffort
    : model.effortLevels.includes('high')
      ? 'high'
      : model.effortLevels[0] ?? 'off';
  return { provider: provider.providerId, model: model.id, reasoningEffort };
}

function researchModelEffort(value: string | null | undefined): ResearchModelEffortLevel | null {
  return value === 'off' || value === 'minimal' || value === 'low' || value === 'medium'
    || value === 'high' || value === 'xhigh' || value === 'max'
    ? value
    : null;
}

export function isReportResourceRun(run: Pick<RunRecord, 'budget'>): boolean {
  const resourceContext = run.budget.resourceContext;
  return Boolean(
    resourceContext &&
    typeof resourceContext === 'object' &&
    'kind' in resourceContext &&
    resourceContext.kind === 'report'
  );
}

export function reportCatalogGroups(reports: readonly AppServerReportSummary[]): {
  complete: AppServerReportSummary[];
  stale: AppServerReportSummary[];
} {
  const complete: AppServerReportSummary[] = [];
  const stale: AppServerReportSummary[] = [];
  for (const report of reports) (report.status === 'stale' ? stale : complete).push(report);
  const newestFirst = (left: AppServerReportSummary, right: AppServerReportSummary): number =>
    right.updatedAt.localeCompare(left.updatedAt) || left.id.localeCompare(right.id);
  complete.sort(newestFirst);
  stale.sort(newestFirst);
  return { complete, stale };
}

export function reportsForReportingScope(
  reports: readonly AppServerReportSummary[],
  workspaceId: string | null
): AppServerReportSummary[] {
  return workspaceId ? reports.filter((report) => report.workspaceId === workspaceId) : [...reports];
}

export interface ReportMarkdownBlock {
  id: string;
  content: string;
  startLine: number;
  endLine: number;
}

export function reportTitleFromMarkdown(content: string, fallback: string): string {
  const firstLine = content.replace(/\r\n?/g, '\n').split('\n', 1)[0]?.trim() ?? '';
  if (!firstLine) return fallback;
  const plain = firstLine
    .replace(/^\s{0,3}#{1,6}\s*/, '')
    .replace(/^\s*>\s*/, '')
    .replace(/^\s*(?:[-+*]|\d+[.)])\s+/, '')
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/<[^>]+>/g, '')
    .replace(/\\([\\`*_{}\[\]()#+\-.!>~|])/g, '$1')
    .replace(/[*_`~|]/g, '')
    .trim();
  return plain || fallback;
}

export function reportMarkdownBlocks(content: string): ReportMarkdownBlock[] {
  const lines = content.replace(/\r\n?/g, '\n').split('\n');
  const blocks: ReportMarkdownBlock[] = [];
  let start = 0;
  let fenced = false;
  const push = (endExclusive: number): void => {
    const block = lines.slice(start, endExclusive).join('\n').trim();
    if (block) {
      blocks.push({
        id: `report-block-${blocks.length + 1}`,
        content: block,
        startLine: start + 1,
        endLine: endExclusive
      });
    }
  };
  for (let index = 0; index <= lines.length; index += 1) {
    const line = lines[index] ?? '';
    if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
    if (index < lines.length && (fenced || line.trim())) continue;
    push(index);
    start = index + 1;
  }
  return blocks;
}

export function replaceReportMarkdownBlock(
  content: string,
  block: Pick<ReportMarkdownBlock, 'startLine' | 'endLine'>,
  replacement: string
): string {
  const lines = content.replace(/\r\n?/g, '\n').split('\n');
  const startIndex = Math.max(0, block.startLine - 1);
  const endIndex = Math.max(startIndex, block.endLine);
  const replacementLines = replacement.replace(/\r\n?/g, '\n').split('\n');
  return [
    ...lines.slice(0, startIndex),
    ...replacementLines,
    ...lines.slice(endIndex)
  ].join('\n');
}

export function reportChangeInstruction(
  block: Pick<ReportMarkdownBlock, 'content' | 'startLine' | 'endLine'>,
  request: string
): string {
  const normalizedRequest = request.trim();
  if (!normalizedRequest) return '';
  const excerpt = [
    `Report lines ${block.startLine}-${block.endLine}:`,
    '```markdown',
    block.content,
    '```'
  ].join('\n');
  return [
    `Selected report block: lines ${block.startLine}-${block.endLine}.`,
    'Editable scope: only this report block. Do not change report content outside this line range.',
    excerpt,
    `Requested edit: ${normalizedRequest}`
  ].join('\n\n');
}
