import type {
  RunDetail,
  RunDetailProjection,
  RunDetailUpdate,
  RunDetailUpdateCursor,
  SubagentPreviewRecord,
  TraceEventRecord,
  TranscriptMessageRecord
} from './types';

const COMMENTARY_EVENT_PAYLOAD_KEYS = [
  'turn',
  'agentId',
  'agentPath',
  'parentAgentId',
  'transcriptRole',
  'transcriptSource',
  'messagePhase',
  'finalResultKind',
  'provider',
  'model',
  'channelName',
  'channel_name',
  'responseId',
  'itemId',
  'linkedTraceEventId',
  'transcriptMessageId',
  'type',
  'action',
  'interruptedByRecovery',
  'fixtureOnly',
  'appServerKind',
  'toolName',
  'appServerSessionEventId',
  'contextUsageEligible',
  'serializedSizeBytes'
] as const;

const COMMENTARY_USAGE_PAYLOAD_KEYS = [
  'input_tokens',
  'inputTokens',
  'input',
  'prompt_tokens',
  'promptTokens',
  'output_tokens',
  'outputTokens',
  'output',
  'completion_tokens',
  'completionTokens',
  'total_tokens',
  'totalTokens',
  'cache_read_tokens',
  'cached_tokens',
  'cacheReadTokens',
  'cacheRead',
  'cache_write_tokens',
  'cacheWriteTokens',
  'cacheWrite',
  'cache_hit_rate',
  'cacheHitRate',
  'source',
  'estimated'
] as const;

const COMMENTARY_CONTENT_PAYLOAD_KEYS = [
  'message',
  'text',
  'outputText',
  'reasoningSummaryTexts'
] as const;

const COMMENTARY_SUBAGENT_PAYLOAD_KEYS = [
  'type',
  'action',
  'agentId',
  'agentPath',
  'parentAgentId',
  'provider',
  'model',
  'channelName',
  'channel_name',
  'status',
  'message'
] as const;

const COMMENTARY_TRANSCRIPT_METADATA_KEYS = [
  'agentId',
  'agentPath',
  'parentAgentId',
  'messagePhase',
  'finalResultKind',
  'provider',
  'model',
  'responseId',
  'itemId',
  'turn',
  'interruptedByRecovery'
] as const;

export function projectRunDetailForRenderer<TDetail extends RunDetail | RunDetailUpdate>(
  detail: TDetail,
  projection: RunDetailProjection,
  sourceCursor?: RunDetailUpdateCursor
): TDetail {
  if (projection === 'full') return detail;
  const targetedAgentPath = typeof projection === 'object' ? projection.agentPath : undefined;
  const traceEvents = targetedAgentPath === undefined
    ? detail.traceEvents
    : detail.traceEvents.filter((event) => includedInAgentCommentaryProjection(event, targetedAgentPath));
  const transcriptMessages = targetedAgentPath === undefined
    ? detail.transcriptMessages
    : detail.transcriptMessages.filter((message) => includedTranscriptInAgentCommentaryProjection(message, targetedAgentPath));
  return {
    ...detail,
    traceEvents: traceEvents.map((event) => projectCommentaryTraceEvent(
      event,
      targetedAgentPath !== undefined && isSubagentActivityEvent(event)
    )),
    transcriptMessages: transcriptMessages.map(projectCommentaryTranscriptMessage),
    ...(targetedAgentPath === undefined ? {} : {
      breakoutRoomMessages: [],
      subagentPreviews: projectedSubagentPreviews(detail.traceEvents, targetedAgentPath),
      projectionCursor: projectedSourceCursor(detail, sourceCursor)
    })
  } as TDetail;
}

function projectedSubagentPreviews(
  events: readonly TraceEventRecord[],
  selectedAgentPath: string | null
): SubagentPreviewRecord[] {
  const latestByPath = new Map<string, SubagentPreviewRecord>();
  for (const event of events) {
    const agentPath = traceEventAgentPath(event);
    if (!agentPath?.startsWith('/root/') || agentPath === selectedAgentPath) continue;
    const message = subagentPreviewMessage(event);
    if (!message) continue;
    const current = latestByPath.get(agentPath);
    if (current && current.sequence > event.sequence) continue;
    latestByPath.set(agentPath, {
      agentPath,
      message: boundedScaffoldText(message),
      sequence: event.sequence,
      createdAt: event.createdAt
    });
  }
  return [...latestByPath.values()].sort((left, right) => left.sequence - right.sequence);
}

function subagentPreviewMessage(event: TraceEventRecord): string | null {
  if (event.payload.transcriptRole !== 'assistant' || isAppServerToolTraceEvent(event)) return null;
  return stringValue(event.payload.text)
    ?? stringValue(event.payload.outputText)
    ?? stringValue(event.payload.message);
}

export function isCommentaryRunDetailProjection(projection: RunDetailProjection): boolean {
  return projection === 'commentary' || typeof projection === 'object';
}

export function runDetailProjectionMetricLabel(projection: RunDetailProjection): string {
  if (typeof projection === 'string') return projection;
  return projection.agentPath === null ? 'commentary-root' : 'commentary-agent';
}

export function projectCommentaryTraceEvent(
  event: TraceEventRecord,
  boundSubagentMessage = false
): TraceEventRecord {
  const payload = pickRecordValues(event.payload, COMMENTARY_EVENT_PAYLOAD_KEYS);
  const metadata = recordValue(event.payload.metadata);
  if (metadata) payload.metadata = pickRecordValues(metadata, COMMENTARY_TRANSCRIPT_METADATA_KEYS);
  const usage = recordValue(event.payload.usage);
  if (usage) payload.usage = boundedRecordValues(usage, COMMENTARY_USAGE_PAYLOAD_KEYS);

  if (isAppServerToolTraceEvent(event)) {
    const toolPayload = recordValue(event.payload.payload);
    payload.payload = toolPayload ? projectToolPayloadScaffold(event, toolPayload) : {};
    payload.commentaryDetailDeferred = true;
  } else if (isCommentaryContentEvent(event)) {
    Object.assign(payload, pickRecordValues(event.payload, COMMENTARY_CONTENT_PAYLOAD_KEYS));
  }

  if (!isAppServerToolTraceEvent(event)) {
    const nestedPayload = recordValue(event.payload.payload);
    const contextScaffold = nestedPayload
      ? nestedPayload.type === 'subagent.activity'
        ? pickRecordValues(nestedPayload, COMMENTARY_SUBAGENT_PAYLOAD_KEYS)
        : pickRecordValues(nestedPayload, ['agentPath', 'contextUsageEligible'] as const)
      : {};
    if (Object.keys(contextScaffold).length > 0) payload.payload = contextScaffold;
  }

  if (boundSubagentMessage) boundSubagentActivityMessages(payload);
  return {
    ...event,
    ...(boundSubagentMessage ? { summary: boundedScaffoldText(event.summary) } : {}),
    payload
  };
}

function includedInAgentCommentaryProjection(event: TraceEventRecord, agentPath: string | null): boolean {
  const eventAgentPath = traceEventAgentPath(event);
  return eventAgentPath === null
    || eventAgentPath === '/root'
    || eventAgentPath === agentPath
    || isSubagentActivityEvent(event)
    || isCollaboratorUsageTelemetryEvent(event, eventAgentPath);
}

function isCollaboratorUsageTelemetryEvent(event: TraceEventRecord, agentPath: string | null): boolean {
  return event.type === 'model_message'
    && Boolean(recordValue(event.payload.usage))
    && Boolean(agentPath?.startsWith('/root/'));
}

function includedTranscriptInAgentCommentaryProjection(
  message: TranscriptMessageRecord,
  agentPath: string | null
): boolean {
  const messageAgentPath = stringValue(message.metadata.agentPath);
  return messageAgentPath === null || messageAgentPath === '/root' || messageAgentPath === agentPath;
}

function traceEventAgentPath(event: TraceEventRecord): string | null {
  const directPath = stringValue(event.payload.agentPath);
  if (directPath) return directPath;
  const nestedPayload = recordValue(event.payload.payload);
  const nestedPath = stringValue(nestedPayload?.agentPath);
  if (nestedPath) return nestedPath;
  return stringValue(recordValue(event.payload.metadata)?.agentPath);
}

function isSubagentActivityEvent(event: TraceEventRecord): boolean {
  if (event.payload.type === 'subagent.activity') return true;
  return recordValue(event.payload.payload)?.type === 'subagent.activity';
}

function boundSubagentActivityMessages(payload: Record<string, unknown>): void {
  delete payload.text;
  delete payload.outputText;
  delete payload.reasoningSummaryTexts;
  if (typeof payload.message === 'string') payload.message = boundedScaffoldValue(payload.message);
  const nestedPayload = recordValue(payload.payload);
  if (nestedPayload && typeof nestedPayload.message === 'string') {
    nestedPayload.message = boundedScaffoldValue(nestedPayload.message);
  }
}

function projectedSourceCursor(
  detail: RunDetail | RunDetailUpdate,
  sourceCursor?: RunDetailUpdateCursor
): RunDetailUpdateCursor {
  const latestTrace = detail.traceEvents.at(-1);
  const afterTraceEventId = typeof latestTrace?.payload.appServerSessionEventId === 'string'
    ? latestTrace.payload.appServerSessionEventId
    : latestTrace?.id ?? sourceCursor?.afterTraceEventId ?? null;
  return {
    afterTraceSequence: latestTrace?.sequence ?? sourceCursor?.afterTraceSequence ?? -1,
    afterTranscriptCount: (sourceCursor?.afterTranscriptCount ?? 0) + detail.transcriptMessages.length,
    afterTraceEventId
  };
}

function projectToolPayloadScaffold(
  event: TraceEventRecord,
  toolPayload: Record<string, unknown>
): Record<string, unknown> {
  const scaffold = pickRecordValues(toolPayload, ['toolActionId', 'toolName', 'status'] as const);
  const toolName = stringValue(event.payload.toolName) ?? stringValue(toolPayload.toolName);
  const inputs = recordValue(toolPayload.normalizedInputs);
  if (toolName && inputs) {
    const inputKeys = toolLabelInputKeys(toolName);
    if (inputKeys.length > 0) scaffold.normalizedInputs = boundedRecordValues(inputs, inputKeys);
  }
  const result = recordValue(toolPayload.result);
  if (toolName && result) {
    const resultKeys = toolLabelResultKeys(toolName);
    if (resultKeys.length > 0) scaffold.result = boundedRecordValues(result, resultKeys);
    if (toolName === 'list_agents' && Array.isArray(result.agents)) {
      const projectedResult = recordValue(scaffold.result) ?? {};
      projectedResult.agents = Array.from({ length: Math.min(1_000, result.agents.length) }, () => null);
      scaffold.result = projectedResult;
    }
  }
  return scaffold;
}

function toolLabelInputKeys(toolName: string): readonly string[] {
  switch (toolName) {
    case 'finding.transition': return ['toStatus'];
    case 'finding.completion_check': return ['targetStatus'];
    case 'investigation.recall':
    case 'finding.list': return ['query'];
    case 'channel_list': return [];
    case 'channel_read': return ['channel_name'];
    case 'resource.catalog': return ['operation'];
    case 'history.search':
    case 'memory.search': return ['query'];
    case 'history.mark_duplicate': return ['type', 'id', 'parentId'];
    case 'history.undo_duplicate': return ['type', 'id'];
    case 'memory.save': return ['type', 'status'];
    case 'memory.link': return ['fromId', 'relation', 'toId'];
    case 'memory.correct': return ['id', 'status'];
    case 'memory.get': return ['id'];
    case 'runbook.list':
    case 'report.list': return ['query'];
    case 'runbook.get': return ['id'];
    case 'runbook.create': return ['title', 'status'];
    case 'runbook.append': return ['id', 'status', 'expectedRevision'];
    case 'runbook.run': return ['id', 'cellId', 'startCellId', 'endCellId'];
    case 'file.read': return ['path'];
    case 'shell.run': return ['command', 'utility', 'args'];
    case 'repository.search': return ['query', 'root', 'path', 'repositoryPath', 'repository'];
    case 'list_agents': return ['path_prefix'];
    case 'wait_agent': return ['timeout_ms'];
    default: return [];
  }
}

function toolLabelResultKeys(toolName: string): readonly string[] {
  switch (toolName) {
    case 'memory.save': return ['id', 'type', 'status', 'revision'];
    case 'runbook.create':
    case 'runbook.append': return ['title', 'status', 'revision'];
    case 'runbook.run': return ['runbookId', 'title'];
    case 'shell.run': return ['command', 'utility', 'args'];
    case 'repository.search': return ['query', 'roots', 'attemptedRoots'];
    case 'list_agents': return [];
    default: return [];
  }
}

function boundedRecordValues(record: Record<string, unknown>, keys: readonly string[]): Record<string, unknown> {
  const bounded: Record<string, unknown> = {};
  for (const key of keys) {
    if (!Object.prototype.hasOwnProperty.call(record, key)) continue;
    const value = boundedScaffoldValue(record[key]);
    if (value !== undefined) bounded[key] = value;
  }
  return bounded;
}

function boundedScaffoldValue(value: unknown): unknown {
  if (typeof value === 'string') return boundedScaffoldText(value);
  if (typeof value === 'number' || typeof value === 'boolean') return value;
  if (Array.isArray(value)) {
    return value.slice(0, 16).flatMap((candidate) => {
      const bounded = boundedScaffoldValue(candidate);
      return bounded === undefined ? [] : [bounded];
    });
  }
  return undefined;
}

function boundedScaffoldText(value: string): string {
  return value.length > 256 ? `${value.slice(0, 255)}…` : value;
}

export function projectCommentaryTranscriptMessage(message: TranscriptMessageRecord): TranscriptMessageRecord {
  return {
    ...message,
    contentMarkdown: message.role === 'system' ? '' : message.contentMarkdown,
    metadata: pickRecordValues(message.metadata, COMMENTARY_TRANSCRIPT_METADATA_KEYS)
  };
}

function isCommentaryContentEvent(event: TraceEventRecord): boolean {
  const role = event.payload.transcriptRole;
  return role === 'user' ||
    role === 'assistant' ||
    event.payload.type === 'subagent.activity' ||
    (event.source === 'model' && event.type === 'model_message' && event.payload.fixtureOnly === true);
}

export function isAppServerToolTraceEvent(event: TraceEventRecord): boolean {
  const kind = event.payload.appServerKind;
  return kind === 'tool.requested' ||
    kind === 'tool.observed' ||
    event.summary.startsWith('app-server tool.requested') ||
    event.summary.startsWith('app-server tool.observed');
}

function pickRecordValues<const TKey extends readonly string[]>(
  record: Record<string, unknown>,
  keys: TKey
): Record<string, unknown> {
  const projected: Record<string, unknown> = {};
  for (const key of keys) {
    if (Object.prototype.hasOwnProperty.call(record, key)) projected[key] = record[key];
  }
  return projected;
}

function recordValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function stringValue(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}
