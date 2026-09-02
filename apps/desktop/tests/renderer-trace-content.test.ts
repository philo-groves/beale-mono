import { describe, expect, it } from 'vitest';
import type { RunDetail, TraceEventRecord } from '@shared/types';
import {
  codeBrowserTracePreview,
  compactTracePath,
  formatReasoningTraceText,
  appServerAgentListResults,
  appServerCollaborationTraceSummary,
  appServerMemoryCorrectionSummary,
  appServerMemoryGetSummary,
  appServerMemoryLinkNote,
  appServerMemorySaveSummary,
  appServerMemorySearchResults,
  appServerShellTraceOutput,
  appServerToolTraceSubtext,
  appServerToolTraceSubtextPill,
  isAppServerToolObservationError,
  isEmptyAppServerMemorySearchObservation,
  isProseTraceEvent,
  lineRangePart,
  pythonTracePreview,
  pythonToolCallPreview,
  reasoningTraceSummariesForEvent,
  reasoningTraceSummariesFromText,
  searchTracePreview,
  traceEventDetailText,
  traceEventSummary,
  verifierTracePreview
} from '../src/renderer/view-models/traceContent';

describe('renderer trace content view models', () => {
  it('normalizes trace summaries into skimmable verb-led labels', () => {
    expect(traceEventSummary(traceEvent({ type: 'model_message', summary: 'OpenAI response completed.' }), 'agent_output')).toBe('Response Completed');
    expect(traceEventSummary(traceEvent({ type: 'model_message', summary: 'OpenAI Responses request sent for turn 12.' }), 'agent_output')).toBe('Request for Turn 12');
    expect(traceEventSummary(traceEvent({ type: 'model_message', summary: 'OpenAI streamed model output delta.' }), 'agent_output')).toBe('Model Output');
    expect(traceEventSummary(traceEvent({ type: 'tool_call', summary: 'app-server tool.requested: repository.search' }), 'tools')).toBe('Repository Search Requested');
    expect(
      traceEventSummary(
        traceEvent({ type: 'tool_result', summary: 'app-server tool.observed: 12 results', payload: { payload: { toolName: 'file.read' } } }),
        'tools'
      )
    ).toBe('File Read');
    expect(traceEventSummary(traceEvent({ type: 'tool_result', summary: 'app-server tool.observed: 12 results' }), 'tools')).toBe('Tool');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_call', summary: 'app-server tool.requested: shell.run', payload: { payload: { toolName: 'shell.run' } } }), 'tools')
    ).toBe('Shell Requested');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_result', summary: 'app-server tool.observed: shell.run', payload: { payload: { toolName: 'shell.run' } } }), 'tools')
    ).toBe('Shell');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_call', summary: 'app-server tool.requested: memory.correct', payload: { payload: { toolName: 'memory.correct' } } }), 'tools')
    ).toBe('Memory Correction Requested');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_result', summary: 'app-server tool.observed: memory.correct', payload: { payload: { toolName: 'memory.correct' } } }), 'tools')
    ).toBe('Memory Correction');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_call', summary: 'app-server tool.requested: runbook.create', payload: { payload: { toolName: 'runbook.create' } } }), 'tools')
    ).toBe('Runbook Creation Requested');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_result', summary: 'app-server tool.observed: runbook.append', payload: { payload: { toolName: 'runbook.append' } } }), 'tools')
    ).toBe('Runbook Update');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_call', summary: 'app-server tool.requested: spawn_agent', payload: { payload: { toolName: 'spawn_agent' } } }), 'tools')
    ).toBe('Spawn Agent Requested');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_result', summary: 'app-server tool.observed: followup_task', payload: { payload: { toolName: 'followup_task' } } }), 'tools')
    ).toBe('Follow-up Task');
    expect(
      traceEventSummary(traceEvent({ type: 'tool_result', summary: 'app-server tool.observed: wait_agent', payload: { payload: { toolName: 'wait_agent' } } }), 'tools')
    ).toBe('Wait for Agent Activity');
    expect(traceEventSummary(traceEvent({ type: 'model_message', summary: 'app-server context.compiled: 53k tokens' }), 'agent_output')).toBe('app-server Context Compiled');
    expect(
      traceEventSummary(
        traceEvent({ type: 'tool_call', summary: 'OpenAI completed function call arguments for python.', payload: { toolName: 'python' } }),
        'tools'
      )
    ).toBe('Prepare Python');
    expect(traceEventSummary(traceEvent({ type: 'tool_call', summary: 'OpenAI requested Beale tool: python.' }), 'tools')).toBe('Queue Python');
    expect(traceEventSummary(traceEvent({ type: 'tool_call', summary: 'OpenAI completed function call arguments for verifier.', payload: { toolName: 'verifier' } }), 'non_standard')).toBe(
      'Prepare Verifier'
    );
    expect(traceEventSummary(traceEvent({ type: 'tool_call', summary: 'OpenAI completed function call arguments for code_browser.', payload: { toolName: 'code_browser' } }), 'non_standard')).toBe(
      'Prepare Code Browser'
    );
    expect(
      traceEventSummary(
        traceEvent({ type: 'tool_call', summary: 'OpenAI completed function call arguments for resource_lookup.', payload: { toolName: 'resource_lookup' } }),
        'non_standard'
      )
    ).toBe('Prepare Resource Lookup');
    expect(traceEventSummary(traceEvent({ type: 'tool_call', summary: 'OpenAI completed function call arguments for search.', payload: { toolName: 'search' } }), 'code_navigation')).toBe(
      'Prepare Search'
    );
    expect(traceEventSummary(traceEvent({ type: 'tool_call', summary: 'OpenAI requested Beale tool: resource_lookup.' }), 'non_standard')).toBe('Queue Resource Lookup');
    expect(traceEventSummary(traceEvent({ type: 'tool_call', summary: 'OpenAI requested Beale tool: search.' }), 'non_standard')).toBe('Queue Search');
    expect(traceEventSummary(traceEvent({ type: 'tool_result', summary: 'Search examined 6 scoped files and returned 40 matches.' }), 'code_navigation')).toBe(
      'Examined 6 files and returned 40 matches'
    );
    expect(traceEventSummary(traceEvent({ type: 'tool_result', summary: 'Examined 1 file and returned 1 match.' }), 'code_navigation')).toBe('Examined 1 file and returned 1 match');
    expect(traceEventSummary(traceEvent({ type: 'tool_result', summary: 'Code browser returned 156 bounded lines.' }), 'code_navigation')).toBe('Read Code');
    expect(traceEventSummary(traceEvent({ type: 'tool_result', summary: 'Code browser could not read the requested bounded text.' }), 'failure_recovery')).toBe('Code Browser Error');
    expect(traceEventSummary(traceEvent({ type: 'artifact_created', summary: 'Artifact recorded: verifier-output.txt.' }), 'artifacts')).toBe('Record artifact: verifier-output.txt');
    expect(traceEventSummary(traceEvent({ type: 'verifier_result', summary: 'Verifier contract executed with pass; finding promotion remains gated.' }), 'verifier')).toBe('Verifier Execution');
    expect(traceEventSummary(traceEvent({ type: 'verifier_result', summary: 'Verifier contract executed on host with pass.' }), 'verifier')).toBe('Verifier Execution');
    expect(traceEventSummary(traceEvent({ type: 'tool_result', summary: 'Host python operation finished with success.' }), 'tools')).toBe('Run Python: success');
    expect(traceEventSummary(traceEvent({ summary: 'Search completed.' }), 'code_navigation')).toBe('Search completed');
    expect(traceEventSummary(traceEvent({ summary: 'Repository status changed.' }), 'events')).toBe('Note: Repository status changed');
  });

  it('shows useful runbook tool metadata', () => {
    expect(appServerToolTraceSubtext(appServerToolRequest('runbook.create', { title: 'VM crash triage', purpose: 'Repeatable proof' }))).toBe('VM crash triage');
    expect(appServerToolTraceSubtext(appServerToolObservation('runbook.append', { id: 'runbook_one', expectedRevision: 2 }, {
      id: 'runbook_one', title: 'VM crash triage', status: 'completed', revision: 3
    }))).toBe('VM crash triage · Update 3');
  });

  it('formats reasoning summaries while preserving summary boundaries', () => {
    expect(formatReasoningTraceText('**Focus** Check parser\nwith range checks\n\n**Risk** Validate index use')).toBe(
      '**Focus**\nCheck parser with range checks\n\n**Risk**\nValidate index use'
    );
    expect(reasoningTraceSummariesFromText('**Focus** Check parser\nwith range checks\n\n**Risk** Validate index use')).toEqual([
      { title: 'Focus', description: 'Check parser with range checks' },
      { title: 'Risk', description: 'Validate index use' }
    ]);
    expect(
      traceEventDetailText(
        traceEvent({
          type: 'model_message',
          source: 'model',
          summary: 'Reasoning.',
          payload: {
            text: '**Focus** Check parser',
            transcriptKind: 'reasoning_summary'
          }
        }),
        'agent_output'
      )
    ).toBe('**Focus**\nCheck parser');
  });

  it('preserves title-only coalesced reasoning summaries and removes repeated partial segments', () => {
    expect(
      reasoningTraceSummariesForEvent(
        traceEvent({
          type: 'model_message',
          source: 'model',
          summary: 'Reasoning.',
          payload: {
            transcriptKind: 'reasoning_summary',
            reasoningSummaryTexts: ['**Inspecting parser**', '**Inspecting parser**\n\n**Checking bounds**', '**Reviewing call sites**']
          }
        }),
        'reasoning'
      )
    ).toEqual([
      { title: 'Inspecting parser', description: '' },
      { title: 'Checking bounds', description: '' },
      { title: 'Reviewing call sites', description: '' }
    ]);
  });

  it('formats key trace detail content without raw JSON noise', () => {
    const search = traceEvent({
      type: 'tool_result',
      source: 'tool',
      summary: 'Search returned results.',
      payload: { query: 'decodeToken', matches: ['a', 'b'], filesConsidered: 14, target: 'auth' }
    });
    expect(traceEventDetailText(search, 'code_navigation')).toBe('query "decodeToken" · 2 matches · files 14 · target auth');

    const appServerToolRequest = traceEvent({
      type: 'tool_call',
      source: 'tool',
      summary: 'app-server tool.requested: repository.search',
      payload: {
        agentId: 'agent_root',
        agentPath: '/root',
        appServerEventId: 'evt_tool_request',
        appServerKind: 'tool.requested',
        appServerSequence: 12,
        payload: {
          toolActionId: 'action_repository_search',
          toolName: 'repository.search',
          normalizedInputs: { query: 'decodeToken' }
        }
      }
    });
    expect(traceEventDetailText(appServerToolRequest, 'code_navigation')).toBe('');

    const appServerToolFailure = traceEvent({
      type: 'tool_result',
      source: 'tool',
      summary: 'app-server tool.observed: Repository search failed.',
      payload: {
        agentId: 'agent_root',
        appServerKind: 'tool.observed',
        appServerSequence: 13,
        payload: {
          toolActionId: 'action_repository_search',
          toolName: 'repository.search',
          status: 'error',
          error: { message: 'Repository is unavailable' }
        }
      }
    });
    expect(traceEventDetailText(appServerToolFailure, 'failure_recovery')).toBe('Repository is unavailable');
    expect(isAppServerToolObservationError(appServerToolFailure)).toBe(true);
    expect(isAppServerToolObservationError(appServerToolRequest)).toBe(false);

    const researchNote = traceEvent({
      type: 'research_event',
      source: 'system',
      summary: 'Fixture research note recorded: ACME challenge middleware bypasses Pages access control.',
      payload: {
        title: 'ACME challenge middleware bypasses Pages access control',
        component: 'challenge middleware',
        description: 'The fixture trace records the research note without creating parallel durable memory.'
      }
    });
    expect(traceEventDetailText(researchNote, 'research')).toBe(
      'title ACME challenge middleware bypasses Pages access control · component challenge middleware · The fixture trace records the research note without creating parallel durable memory.'
    );
    expect(isProseTraceEvent(researchNote, 'research')).toBe(true);
  });

  it('renders collaboration tool targets, prompts, wait state, and bounded agent lists', () => {
    const spawn = appServerToolObservation(
      'spawn_agent',
      { task_name: 'modules', message: 'Audit module entry points.\nPrioritize default loading.', fork_turns: '2', model: 'gpt-5.6-sol', reasoning_effort: 'high' },
      { agent_id: 'agent_1', task_name: '/root/modules', model: 'gpt-5.6-sol', reasoning_effort: 'high', fork_turns: '2' }
    );
    const followup = appServerToolObservation(
      'followup_task',
      { target: 'modules', message: 'Check the remaining parser path.' },
      { delivered: true, target: '/root/modules', triggered_turn: true }
    );
    const interrupt = appServerToolObservation('interrupt_agent', { target: 'modules' }, { target: '/root/modules', previous_status: 'running' });
    const wait = appServerToolObservation('wait_agent', { timeout_ms: 5000 }, { message: 'Agent activity is ready.', timed_out: false });
    const list = appServerToolObservation(
      'list_agents',
      {},
      {
        agents: Array.from({ length: 7 }, (_, index) => ({
          id: `agent_${index}`,
          path: `/root/worker_${index}`,
          status: index === 0 ? 'running' : 'completed',
          model: 'gpt-5.6-sol',
          reasoning_effort: 'high'
        }))
      }
    );

    expect(appServerToolTraceSubtext(spawn)).toBe('/root/modules · Last 2 turns · gpt-5.6-sol · High effort');
    expect(appServerCollaborationTraceSummary(spawn)).toBe('Audit module entry points.\nPrioritize default loading.');
    expect(appServerToolTraceSubtext(followup)).toBe('/root/modules · Turn started');
    expect(appServerCollaborationTraceSummary(followup)).toBe('Check the remaining parser path.');
    expect(appServerToolTraceSubtext(interrupt)).toBe('/root/modules · Was Running');
    expect(appServerToolTraceSubtext(wait)).toBe('5s timeout');
    expect(appServerCollaborationTraceSummary(wait)).toBe('Agent activity is ready.');
    expect(appServerToolTraceSubtext(list)).toBe('All agents · 7 agents');
    expect(appServerAgentListResults(list)).toEqual({
      rows: [
        '/root/worker_0 · Running · gpt-5.6-sol · High effort',
        '/root/worker_1 · Completed · gpt-5.6-sol · High effort',
        '/root/worker_2 · Completed · gpt-5.6-sol · High effort',
        '/root/worker_3 · Completed · gpt-5.6-sol · High effort',
        '/root/worker_4 · Completed · gpt-5.6-sol · High effort'
      ],
      allRows: Array.from({ length: 7 }, (_, index) => `/root/worker_${index} · ${index === 0 ? 'Running' : 'Completed'} · gpt-5.6-sol · High effort`),
      count: 7
    });
  });

  it('extracts separate stdout and stderr streams from app-server shell observations', () => {
    const shellRun = appServerToolObservation(
      'shell.run',
      { utility: 'make', args: ['test'] },
      {
        exitCode: 1,
        stdout: 'building\r\nfinished\n',
        stderr: 'test failed\r\nline two\n',
        stdoutTruncated: false,
        stderrTruncated: true
      }
    );

    expect(appServerShellTraceOutput(shellRun)).toEqual({
      stdout: { lines: ['building', 'finished'], allLines: ['building', 'finished'], lineCount: 2, sourceTruncated: false, truncated: false },
      stderr: 'test failed\nline two\n',
      stderrTruncated: true
    });
    expect(
      appServerShellTraceOutput(
        appServerToolObservation('shell.run', { utility: 'printf', args: [] }, { stdout: 'one\ntwo\nthree\nfour\nfive\nsix\n', stderr: '' })
      )
    ).toEqual({
      stdout: {
        lines: ['one', 'two', 'three', 'four', 'five'],
        allLines: ['one', 'two', 'three', 'four', 'five', 'six'],
        lineCount: 6,
        sourceTruncated: false,
        truncated: true
      },
      stderr: '',
      stderrTruncated: false
    });
    expect(appServerShellTraceOutput(appServerToolRequest('shell.run', { utility: 'make', args: ['test'] }))).toBeNull();
    expect(appServerShellTraceOutput(appServerToolObservation('history.search', { query: 'test' }, { results: [] }))).toBeNull();
  });

  it('builds python previews and prose decisions for trace rows', () => {
    const python = traceEvent({
      type: 'tool_call',
      summary: 'OpenAI completed function call arguments for python.',
      payload: {
        toolName: 'python',
        arguments: {
          task: 'Check parser edge cases',
          script: Array.from({ length: 10 }, (_, index) => `print(${index})`).join('\n')
        }
      }
    });

    expect(pythonToolCallPreview(python)).toMatchObject({
      task: 'Check parser edge cases',
      scriptLines: ['print(0)', 'print(1)', 'print(2)', 'print(3)', 'print(4)'],
      scriptLineCount: 10,
      truncated: true,
      outputLines: []
    });
    expect(pythonToolCallPreview(python, 8)).toMatchObject({
      scriptLines: ['print(0)', 'print(1)', 'print(2)', 'print(3)', 'print(4)', 'print(5)', 'print(6)', 'print(7)'],
      scriptLineCount: 10,
      truncated: true
    });

    const result = traceEvent({
      id: 'trace_result',
      type: 'tool_result',
      summary: 'Host python operation finished with success.',
      toolCallId: 'tool_python',
      payload: { exitCode: 0, stdoutSummary: 'ok\nnext', stderrSummary: '' }
    });
    const detail = runDetail({
      traceEvents: [
        traceEvent({
          id: 'trace_tool_call',
          type: 'tool_call',
          summary: 'OpenAI requested Beale tool: python.',
          toolCallId: 'tool_python',
          payload: python.payload
        }),
        result
      ]
    });
    expect(pythonTracePreview(result, detail)).toMatchObject({
      task: 'Check parser edge cases',
      scriptLines: ['print(0)', 'print(1)', 'print(2)', 'print(3)', 'print(4)'],
      scriptLineCount: 10,
      truncated: true,
      outputLines: ['ok', 'next'],
      outputLineCount: 2,
      outputTruncated: false,
      exitCode: '0'
    });

    expect(
      pythonTracePreview(
        traceEvent({
          id: 'trace_no_output',
          type: 'tool_result',
          summary: 'Host python operation finished with success.',
          toolCallId: 'tool_python',
          payload: { exitCode: 0 }
        }),
        detail
      )
    ).toMatchObject({
      outputLines: ['No output recorded.'],
      exitCode: '0'
    });
    expect(isProseTraceEvent(traceEvent({ source: 'model', type: 'model_message', payload: { text: 'Agent response', transcriptRole: 'assistant' } }), 'agent_output')).toBe(true);
    expect(lineRangePart({ lineStart: 12, lineEnd: 19 })).toBe('lines 12-19');
  });

  it('projects typed workspace history search results while retaining legacy memory-search traces', () => {
    const history = appServerToolObservation('history.search', { query: 'parser' }, {
      resultCount: 2,
      results: [
        { type: 'claim', id: 'claim_parser', title: 'Parser allocation' },
        { type: 'runbook', id: 'runbook_parser', title: 'Parser reproduction' }
      ]
    });
    expect(appServerMemorySearchResults(history)).toMatchObject({
      allTitles: ['Parser allocation', 'Parser reproduction'],
      resultCount: 2,
      truncated: false
    });
    expect(isEmptyAppServerMemorySearchObservation(history)).toBe(false);
    expect(isEmptyAppServerMemorySearchObservation(
      appServerToolObservation('history.search', { types: ['claims'] }, { results: [] })
    )).toBe(true);
    expect(isEmptyAppServerMemorySearchObservation(
      appServerToolObservation('memory.search', { query: 'legacy' }, [])
    )).toBe(true);
  });

  it('builds structured verifier previews without raw id-heavy detail text', () => {
    expect(
      verifierTracePreview(
        traceEvent({
          type: 'verifier_result',
          source: 'verifier',
          summary: 'Verifier contract executed on host with pass.',
          payload: {
            status: 'pass',
            realExecution: true,
            hostExecution: true,
            vmExecution: false,
            artifactId: 'artifact_test',
            verifierRunId: 'verifier_run_test',
            contractId: 'verifier_contract_test'
          }
        })
      )
    ).toEqual({
      title: 'PASS',
      description: 'Host verifier · real execution · output artifact recorded',
      facts: []
    });

  });

  it('builds structured code browser previews from bounded excerpts', () => {
    expect(
      codeBrowserTracePreview(
        traceEvent({
          type: 'tool_result',
          source: 'tool',
          summary: 'Code browser returned 12 bounded lines.',
          payload: {
            sourcePath: '/repo/services/payments/src/main/java/com/example/security/Decoder.java',
            lineStart: 10,
            lineEnd: 21,
            symbol: 'decode',
            truncated: true,
            excerpt: ['10: public void decode() {', '11:   parse(input);', '12: }', '13:', '14: // extra', '15: audit();'].join('\n')
          }
        })
      )
    ).toEqual({
      title: '.../example/security/Decoder.java',
      description: '',
      facts: ['lines 10-21', '12 lines', 'symbol decode', 'truncated yes'],
      excerptLines: ['10: public void decode() {', '11:   parse(input);', '12: }', '13:', '14: // extra'],
      excerptAllLines: ['10: public void decode() {', '11:   parse(input);', '12: }', '13:', '14: // extra', '15: audit();'],
      excerptLineCount: 12,
      excerptSourceTruncated: true,
      excerptTruncated: true
    });
    expect(
      codeBrowserTracePreview(
        traceEvent({
          type: 'tool_result',
          source: 'tool',
          summary: 'Code browser returned 12 bounded lines.',
          payload: {
            sourcePath: '/repo/services/payments/src/main/java/com/example/security/Decoder.java',
            excerpt: ['1: a', '2: b', '3: c', '4: d', '5: e', '6: f'].join('\n')
          }
        }),
        8
      )?.excerptLines
    ).toEqual(['1: a', '2: b', '3: c', '4: d', '5: e', '6: f']);
  });

  it('builds structured file-read previews from app-server tool observations', () => {
    expect(
      codeBrowserTracePreview(
        traceEvent({
          type: 'tool_result',
          source: 'tool',
          summary: 'app-server tool.observed: Read 128 byte(s) from /repo/Src/parse.c with truncation.',
          payload: {
            appServerKind: 'tool.observed',
            payload: {
              toolName: 'file.read',
              status: 'complete',
              normalizedInputs: { path: '/repo/Src/parse.c', maxBytes: 128 },
              result: {
                requestedPath: '/repo/Src/parse.c',
                resolvedPath: '/repo/Src/parse.c',
                offset: 0,
                bytesRead: 128,
                totalBytes: 512,
                truncated: true,
                encoding: 'utf8',
                containsNulByte: false,
                text: ['parse one', 'parse two', 'parse three', 'parse four', 'parse five', 'parse six'].join('\n')
              }
            }
          }
        })
      )
    ).toEqual({
      title: '/repo/Src/parse.c',
      description: '',
      facts: ['128 bytes', 'utf8', 'truncated yes'],
      excerptLines: ['parse one', 'parse two', 'parse three', 'parse four', 'parse five'],
      excerptAllLines: ['parse one', 'parse two', 'parse three', 'parse four', 'parse five', 'parse six'],
      excerptLineCount: 6,
      excerptSourceTruncated: true,
      excerptTruncated: true
    });
  });

  it('builds structured search previews from ranked search results', () => {
    expect(
      searchTracePreview(
        traceEvent({
          type: 'tool_result',
          source: 'tool',
          summary: 'Examined 47 files and returned 31 matches.',
          payload: {
            query: 'auth middleware bypass',
            filesConsidered: 47,
            targetHint: '/repo/apps/api',
            metadataMatches: 4,
            semanticMatches: 2,
            graphMatches: 3,
            matches: ['direct']
          }
        })
      )
    ).toEqual({
      title: 'Search auth middleware bypass',
      description: '31 matches',
      facts: ['47 files', '/repo/apps/api', '4 metadata', '2 semantic', '3 graph']
    });
  });

  it('uses recorded event payloads for research and artifact trace details', () => {
    const researchEvent = traceEvent({
      id: 'trace_research',
      type: 'research_event',
      payload: { title: 'Parser boundary note', description: 'Unchecked arithmetic is visible in the captured source excerpt.' }
    });
    const artifactEvent = traceEvent({
      id: 'trace_artifact',
      type: 'artifact_created',
      payload: { title: 'Verifier output', artifactId: 'artifact_one', status: 'captured' }
    });

    expect(traceEventDetailText(researchEvent, 'research')).toBe(
      'title Parser boundary note · Unchecked arithmetic is visible in the captured source excerpt.'
    );
    expect(traceEventDetailText(artifactEvent, 'artifacts')).toBe('artifact artifact_one');
  });

  it('compacts long trace paths from the right-hand side', () => {
    expect(compactTracePath('/repo/services/payments/src/main/java/com/example/security/Decoder.java')).toBe('.../example/security/Decoder.java');
  });
});

function runDetail(
  input: { traceEvents?: TraceEventRecord[]; appServerMemory?: RunDetail['appServerMemory'] } = {}
): RunDetail {
  return {
    run: {
      id: 'run_test',
      status: 'completed',
      createdAt: '2026-04-30T10:00:00.000Z',
      startedAt: '2026-04-30T10:00:00.000Z',
      endedAt: null,
      mode: 'dynamic',
      attemptStrategy: 'breadth_first',
      title: '',
      promptMarkdown: ''
    },
    attempts: [],
    traceEvents: input.traceEvents ?? [],
    transcriptMessages: [],
    hypotheses: [],
    artifacts: [],
    evidence: [],
    findings: [],
    verifierContracts: [],
    verifierRuns: [],
    modelSessions: [],
    contextCompactions: [],
    policyEvents: [],
    exports: [],
    ...(input.appServerMemory ? { appServerMemory: input.appServerMemory } : {})
  } as unknown as RunDetail;
}

function appServerToolRequest(toolName: string, normalizedInputs: Record<string, unknown>): TraceEventRecord {
  return traceEvent({
    source: 'model',
    type: 'tool_call',
    summary: `app-server tool.requested: ${toolName}`,
    payload: {
      agentPath: '/root',
      appServerKind: 'tool.requested',
      payload: {
        toolActionId: `action_${toolName}`,
        toolName,
        normalizedInputs
      }
    }
  });
}

function appServerToolObservation(toolName: string, normalizedInputs: Record<string, unknown>, result: unknown): TraceEventRecord {
  return traceEvent({
    source: 'tool',
    type: 'tool_result',
    summary: `app-server tool.observed: ${toolName}`,
    payload: {
      agentPath: '/root',
      appServerKind: 'tool.observed',
      payload: {
        toolActionId: `action_${toolName}`,
        toolName,
        normalizedInputs,
        status: 'complete',
        result
      }
    }
  });
}

function traceEvent(input: Partial<TraceEventRecord> = {}): TraceEventRecord {
  return {
    id: 'trace_test',
    runId: 'run_test',
    attemptId: null,
    sequence: 1,
    source: 'system',
    type: 'user_note',
    summary: 'Trace event.',
    payload: {},
    sensitivity: 'internal',
    modelVisible: true,
    createdAt: '2026-04-30T10:00:00.000Z',
    artifactId: null,
    toolCallId: null,
    approvalId: null,
    ...input
  };
}
