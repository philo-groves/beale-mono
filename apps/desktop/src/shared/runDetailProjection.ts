import type {
  RunDetail,
  RunDetailProjection,
  RunDetailUpdate,
  RunDetailUpdateCursor,
  RunHistoryPage,
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
  'responseId',
  'itemId',
  'linkedTraceEventId',
  'transcriptMessageId',
  'type',
  'action',
  'authorAgentPath',
  'recipientAgentPath',
  'participantAgentPaths',
  'roomName',
  'roomTitle',
  'packetKind',
  'recoveryKind',
  'awaitingSteering',
  'contextPhase',
  'interruptedByRecovery',
  'fixtureOnly',
  'appServerKind',
  'toolName',
  'appServerSessionEventId',
  'contextUsageEligible',
  'contextWindow',
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
  'authorAgentPath',
  'recipientAgentPath',
  'participantAgentPaths',
  'roomName',
  'roomTitle',
  'packetKind',
  'agentId',
  'agentPath',
  'parentAgentId',
  'provider',
  'model',
  'status',
  'message'
] as const;

const COMMENTARY_MODEL_RETRY_PAYLOAD_KEYS = [
  'type',
  'agentId',
  'agentPath',
  'parentAgentId',
  'recoveryKind',
  'awaitingSteering',
  'contextPhase'
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
  const legacyAuthors = legacyMessageAuthors(detail.traceEvents);
  const traceEvents = targetedAgentPath === undefined
    ? detail.traceEvents
    : detail.traceEvents.filter((event) => includedInAgentCommentaryProjection(event, targetedAgentPath));
  const transcriptMessages = targetedAgentPath === undefined
    ? detail.transcriptMessages
    : detail.transcriptMessages.filter((message) => includedTranscriptInAgentCommentaryProjection(message, targetedAgentPath));
  return {
    ...detail,
    traceEvents: traceEvents.map((event) => projectCommentaryTraceEvent(
      withLegacyMessageAuthor(event, legacyAuthors.get(event.id)),
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

export function projectRunHistoryPageForRenderer(page: RunHistoryPage, projection: RunDetailProjection): RunHistoryPage {
  if (projection === 'full') return page;
  const targetedAgentPath = typeof projection === 'object' ? projection.agentPath : undefined;
  const legacyAuthors = legacyMessageAuthors(page.traceEvents);
  const traceEvents = targetedAgentPath === undefined
    ? page.traceEvents
    : page.traceEvents.filter((event) => includedInAgentCommentaryProjection(event, targetedAgentPath));
  const transcriptMessages = targetedAgentPath === undefined
    ? page.transcriptMessages
    : page.transcriptMessages.filter((message) => includedTranscriptInAgentCommentaryProjection(message, targetedAgentPath));
  return {
    ...page,
    traceEvents: traceEvents.map((event) => projectCommentaryTraceEvent(
      withLegacyMessageAuthor(event, legacyAuthors.get(event.id)),
      targetedAgentPath !== undefined && isSubagentActivityEvent(event)
    )),
    transcriptMessages: transcriptMessages.map(projectCommentaryTranscriptMessage)
  };
}

function legacyMessageAuthors(events: readonly TraceEventRecord[]): Map<string, string> {
  const requestsByMessage = new Map<string, Array<{ event: TraceEventRecord; author: string }>>();
  for (const event of events) {
    if (event.payload.appServerKind !== 'tool.requested' || event.payload.toolName !== 'send_message') continue;
    const input = recordValue(recordValue(event.payload.payload)?.normalizedInputs);
    const target = stringValue(input?.target);
    const message = stringValue(input?.message);
    const author = traceEventAgentPath(event);
    if ((target !== 'root' && target !== '/root') || !message || !author?.startsWith('/root/')) continue;
    const requests = requestsByMessage.get(message) ?? [];
    requests.push({ event, author });
    requestsByMessage.set(message, requests);
  }
  const authors = new Map<string, string>();
  for (const event of events) {
    const activity = event.payload.type === 'subagent.activity'
      ? event.payload
      : recordValue(event.payload.payload);
    if (activity?.type !== 'subagent.activity' || activity.action !== 'message'
      || stringValue(activity.authorAgentPath)) continue;
    const recipient = stringValue(activity.recipientAgentPath) ?? stringValue(activity.agentPath);
    const message = stringValue(activity.message);
    if (recipient !== '/root' || !message) continue;
    const activityTime = Date.parse(event.createdAt);
    if (!Number.isFinite(activityTime)) continue;
    const matching = (requestsByMessage.get(message) ?? []).filter(({ event: request }) => {
      const requestTime = Date.parse(request.createdAt);
      return request.attemptId === event.attemptId
        && request.sequence < event.sequence
        && Number.isFinite(requestTime)
        && activityTime - requestTime >= 0
        && activityTime - requestTime <= 60_000;
    });
    const distinctAuthors = new Set(matching.map(({ author }) => author));
    if (distinctAuthors.size === 1) authors.set(event.id, matching[0]!.author);
  }
  return authors;
}

function withLegacyMessageAuthor(event: TraceEventRecord, author: string | undefined): TraceEventRecord {
  if (!author) return event;
  const direct = event.payload.type === 'subagent.activity';
  const activity = direct ? event.payload : recordValue(event.payload.payload);
  if (!activity || stringValue(activity.authorAgentPath)) return event;
  return {
    ...event,
    payload: direct
      ? { ...event.payload, authorAgentPath: author }
      : { ...event.payload, payload: { ...activity, authorAgentPath: author } }
  };
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
  if (event.payload.type === 'research_checkpoint' && event.payload.reason === 'native') {
    payload.reason = 'native';
  }
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
        : nestedPayload.type === 'model_retry'
          ? pickRecordValues(nestedPayload, COMMENTARY_MODEL_RETRY_PAYLOAD_KEYS)
        : nestedPayload.type === 'track_epoch_resolving' || nestedPayload.type === 'track_epoch_paused'
          ? pickRecordValues(nestedPayload, ['type', 'cause', 'agentPath'] as const)
        : pickRecordValues(nestedPayload, ['agentPath', 'contextUsageEligible'] as const)
      : {};
    if (Object.keys(contextScaffold).length > 0) payload.payload = contextScaffold;
    if (isSubagentActivityEvent(event)) {
      const surfaceTrail = projectedSurfaceTrail(event.payload.surfaceTrail ?? nestedPayload?.surfaceTrail);
      if (surfaceTrail.length > 0) {
        if (event.payload.type === 'subagent.activity') payload.surfaceTrail = surfaceTrail;
        else if (recordValue(payload.payload)) (payload.payload as Record<string, unknown>).surfaceTrail = surfaceTrail;
      }
    }
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
    || isSurfaceFocusObservation(event)
    || isSurfaceDiagramObservation(event)
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
  if (typeof payload.message === 'string') payload.message = boundedSubagentActivityText(payload, payload.message);
  const nestedPayload = recordValue(payload.payload);
  if (nestedPayload && typeof nestedPayload.message === 'string') {
    nestedPayload.message = boundedSubagentActivityText(nestedPayload, nestedPayload.message);
  }
}

function boundedSubagentActivityText(payload: Record<string, unknown>, message: string): string {
  const action = stringValue(payload.action);
  const limit = action === 'message' || action === 'followup' || action === 'room_packet' ? 8_192 : 256;
  return message.length > limit ? `${message.slice(0, limit - 1)}…` : message;
}

function projectedSourceCursor(
  detail: RunDetail | RunDetailUpdate,
  sourceCursor?: RunDetailUpdateCursor
): RunDetailUpdateCursor {
  const latestTrace = detail.traceEvents.at(-1);
  const afterTraceEventId = detail.projectionCursor
    ? detail.projectionCursor.afterTraceEventId ?? null
    : typeof latestTrace?.payload.appServerSessionEventId === 'string'
      ? latestTrace.payload.appServerSessionEventId
      : latestTrace?.id ?? sourceCursor?.afterTraceEventId ?? null;
  return {
    afterTraceSequence: detail.projectionCursor?.afterTraceSequence ?? latestTrace?.sequence ?? sourceCursor?.afterTraceSequence ?? -1,
    afterTranscriptCount: detail.projectionCursor?.afterTranscriptCount
      ?? (sourceCursor?.afterTranscriptCount ?? 0) + detail.transcriptMessages.length,
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
    if (toolName === 'surface.focus' && toolPayload.status === 'complete') {
      const surfaceTrail = projectedSurfaceTrail(result.surfaceTrail);
      if (surfaceTrail.length > 0) scaffold.result = { surfaceTrail };
    }
  }
  const surfaceDiagram = surfaceDiagramFromToolObservation(event, toolPayload);
  if (surfaceDiagram) scaffold.surfaceDiagram = surfaceDiagram;
  return scaffold;
}

function isSurfaceDiagramObservation(event: TraceEventRecord): boolean {
  return surfaceDiagramFromToolObservation(event, recordValue(event.payload.payload) ?? {}) !== null;
}

function surfaceDiagramFromToolObservation(event: TraceEventRecord, toolPayload: Record<string, unknown>):
  { source: string; surfaceId: string; surfaceName: string; changeKind?: 'created' | 'updated' } | null {
  if (event.payload.appServerKind !== 'tool.observed' || toolPayload.status !== 'complete'
    || (stringValue(event.payload.toolName) ?? stringValue(toolPayload.toolName)) !== 'surface.update') return null;
  const observed = recordValue(toolPayload.surfaceDiagram);
  const inputs = recordValue(toolPayload.normalizedInputs);
  const foundation = recordValue(inputs?.foundation);
  const threatModel = recordValue(foundation?.threatModel);
  const source = stringValue(observed?.source) ?? stringValue(threatModel?.diagram);
  const surfaceId = stringValue(observed?.surfaceId) ?? stringValue(inputs?.surfaceId);
  if (!source?.trim() || source.length > 50_000 || !surfaceId) return null;
  const result = recordValue(toolPayload.result);
  const changeKind = stringValue(observed?.changeKind) ?? stringValue(result?.diagramChange);
  return { source, surfaceId,
    surfaceName: boundedScaffoldText(stringValue(observed?.surfaceName) ?? stringValue(result?.name) ?? stringValue(inputs?.name) ?? 'Surface'),
    ...(changeKind === 'created' || changeKind === 'updated' ? { changeKind } : {}) };
}

function isSurfaceFocusObservation(event: TraceEventRecord): boolean {
  return event.payload.appServerKind === 'tool.observed'
    && (event.payload.toolName === 'surface.focus'
      || recordValue(event.payload.payload)?.toolName === 'surface.focus');
}

function projectedSurfaceTrail(value: unknown): Array<{ id: string; name: string }> {
  if (!Array.isArray(value)) return [];
  return value.slice(0, 64).flatMap((item) => {
    const record = recordValue(item);
    const id = stringValue(record?.id);
    const name = stringValue(record?.name);
    return id && name ? [{ id: boundedScaffoldText(id), name: boundedScaffoldText(name) }] : [];
  });
}

function toolLabelInputKeys(toolName: string): readonly string[] {
  switch (toolName) {
    case 'finding.transition': return ['toStatus'];
    case 'finding.completion_check': return ['targetStatus'];
    case 'finding.list': return ['query'];
    case 'resource.catalog': return ['operation', 'name', 'resourceId', 'historyId'];
    case 'history.search':
    case 'memory.search': return ['query'];
    case 'history.mark_duplicate': return ['type', 'id', 'parentId'];
    case 'history.undo_duplicate': return ['type', 'id'];
    case 'memory.save': return ['type', 'title', 'status'];
    case 'memory.link': return ['fromId', 'relation', 'toId'];
    case 'memory.correct': return ['id', 'status'];
    case 'memory.get': return ['id'];
    case 'runbook.list':
    case 'report.list': return ['query'];
    case 'runbook.get': return ['id'];
    case 'runbook.prepare': return ['title', 'candidatePath'];
    case 'runbook.create': return ['title', 'status'];
    case 'runbook.append': return ['id', 'status', 'expectedRevision'];
    case 'runbook.edit': return ['id', 'cellId', 'expectedRevision'];
    case 'runbook.run': return ['id', 'cellId', 'startCellId', 'endCellId'];
    case 'file.read':
    case 'file.edit': return ['path'];
    case 'surface.views': return ['surfaceId', 'views'];
    case 'surface.get':
    case 'surface.breakdown':
    case 'surface.recon':
    case 'surface.analysis':
    case 'surface.threat_model':
    case 'surface.resources':
    case 'surface.focus': return ['surfaceId'];
    case 'surface.list': return ['parentId', 'resourceId'];
    case 'surface.search':
    case 'surface.query': return ['query'];
    case 'features.load': return ['features'];
    case 'plugins.load': return ['plugins'];
    case 'features.preview': return ['feature'];
    case 'plugins.preview': return ['plugin'];
    case 'shell.run': return ['command', 'utility', 'args'];
    case 'repository.search': return ['query', 'root', 'path', 'repositoryPath', 'repository'];
    case 'workspace.search': return ['workspaceId', 'query', 'path', 'mode', 'categories', 'extensions', 'modifiedAfter', 'includeRaw', 'includeTemporary', 'includeCanonicalExports', 'offset', 'maxResults'];
    case 'list_agents': return ['path_prefix'];
    case 'wait_agent': return ['timeout_ms'];
    default: return [];
  }
}

function toolLabelResultKeys(toolName: string): readonly string[] {
  switch (toolName) {
    case 'memory.save': return ['id', 'type', 'title', 'status', 'revision'];
    case 'runbook.create':
    case 'runbook.append':
    case 'runbook.edit': return ['title', 'status', 'revision'];
    case 'runbook.prepare': return ['runbookId', 'entryCellId', 'candidatePath', 'revision'];
    case 'runbook.run': return ['runbookId', 'title'];
    case 'shell.run': return ['command', 'utility', 'args'];
    case 'repository.search': return ['query', 'roots', 'attemptedRoots'];
    case 'surface.get':
    case 'surface.views':
    case 'surface.breakdown':
    case 'surface.recon':
    case 'surface.analysis':
    case 'surface.threat_model':
    case 'surface.resources': return ['name'];
    case 'workspace.search': return ['workspace', 'query', 'matches', 'nextOffset', 'visitedFiles', 'partial'];
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
