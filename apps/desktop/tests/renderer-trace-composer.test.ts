import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { ApprovalRecord, ResearchProviderModelCatalog, RunDetail, RunStatus, TraceEventRecord } from '@shared/types';
import {
  CollaborationSettingsForm,
  MainSteerArea,
  SHELL_SAFETY_MODE_OPTIONS,
  STEER_TEXTAREA_DEFAULT_EXTRA_LINES,
  STEER_TEXTAREA_MAX_LINES,
  selectNextCollaborationRoute,
  steeringSafetyModeOptions
} from '../src/renderer/features/sessions/SessionComposer';
import {
  shortSteeringSuggestion,
  steeringInputSuggestion,
  steeringInputTabAction
} from '../src/renderer/view-models/steeringSuggestions';

describe('renderer session composer', () => {
  it('allows the steering input to grow through seven typed lines', () => {
    expect(STEER_TEXTAREA_MAX_LINES).toBe(7);
  });

  it('adds one typed row to the steering input resting height', () => {
    expect(STEER_TEXTAREA_DEFAULT_EXTRA_LINES).toBe(1);
  });

  it('replaces Send with Stop while the session is active', () => {
    const html = renderTraceComposer('active');

    expect(html).toContain('aria-label="Stop session"');
    expect(html).not.toContain('aria-label="Send steering instruction"');
    expect(html).toContain('placeholder="Steer the research"');
  });

  it('shows Send after the session is no longer active', () => {
    const html = renderTraceComposer('stopped');

    expect(html).toContain('aria-label="Send steering instruction"');
    expect(html).not.toContain('aria-label="Stop session"');
    expect(html).toContain('placeholder="Resume from the last useful result."');
  });

  it('shows a current-session continuation suggestion when the run has ended', () => {
    const html = renderTraceComposer('completed', {
      transcriptMessages: [{
        id: 'final_message',
        runId: 'run_composer',
        attemptId: 'attempt_one',
        traceEventId: 'trace_final',
        role: 'assistant',
        phase: 'final_answer',
        contentMarkdown: 'Final result.',
        source: 'honeycrisp',
        metadata: {
          nextPromptSuggestions: [{
            title: 'Validate crash',
            promptMarkdown: 'Inspect the saved crash and validate the suspected bounds check.'
          }]
        },
        createdAt: '2026-08-14T10:00:00.000Z'
      }]
    });

    expect(html).toContain('placeholder="Inspect the saved crash and validate the suspected bounds check."');
  });

  it('hides response suggestions when they are disabled', () => {
    const html = renderToStaticMarkup(createElement(MainSteerArea, {
      busy: false,
      detail: composerDetail('completed', {
        transcriptMessages: [{
          id: 'final_message',
          runId: 'run_composer',
          attemptId: 'attempt_one',
          traceEventId: 'trace_final',
          role: 'assistant',
          phase: 'final_answer',
          contentMarkdown: 'Final result.',
          source: 'honeycrisp',
          metadata: {
            nextPromptSuggestions: [{
              title: 'Validate crash',
              promptMarkdown: 'Inspect the saved crash and validate the suspected bounds check.'
            }]
          },
          createdAt: '2026-08-14T10:00:00.000Z'
        }]
      }),
      providerModelCatalog: providerModelCatalog(),
      responseSuggestionsEnabled: false,
      runId: 'run_composer',
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).not.toContain('Inspect the saved crash and validate the suspected bounds check.');
    expect(html).toContain('placeholder="Your move"');
  });

  it('uses Tab to first show and then accept an input suggestion', () => {
    expect(steeringInputTabAction({
      instruction: '',
      suggestion: 'Continue from the latest findings.',
      suggestionShowing: false
    })).toBe('show_suggestion');
    expect(steeringInputTabAction({
      instruction: '',
      suggestion: 'Continue from the latest findings.',
      suggestionShowing: true
    })).toBe('accept_suggestion');
    expect(steeringInputTabAction({
      instruction: 'manual text',
      suggestion: 'Continue from the latest findings.',
      suggestionShowing: true
    })).toBe('none');
  });

  it('shows an explicit initial suggestion immediately so the first Tab accepts it', () => {
    const html = renderToStaticMarkup(createElement(MainSteerArea, {
      runId: null,
      detail: null,
      providerModelCatalog: providerModelCatalog(),
      busy: false,
      initialSuggestion: 'Review this report.',
      onInitialInstruction: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).toContain('placeholder="Review this report."');
    expect(html).toMatch(/aria-label="Shell safety mode" aria-haspopup="listbox" aria-expanded="false"><svg[^>]*main-steer-safety-mode-icon/u);
    expect(steeringInputTabAction({
      instruction: '',
      suggestion: 'Review this report.',
      suggestionShowing: true
    })).toBe('accept_suggestion');
  });

  it('supports a view-specific input placeholder', () => {
    const html = renderToStaticMarkup(createElement(MainSteerArea, {
      runId: null,
      detail: null,
      providerModelCatalog: providerModelCatalog(),
      busy: false,
      inputPlaceholder: 'Write a full research prompt',
      onInitialInstruction: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).toContain('placeholder="Write a full research prompt"');
  });

  it('keeps steering suggestions under fifteen words', () => {
    const suggestion = shortSteeringSuggestion(
      'Continue by validating the parser crash with saved artifacts and then compare adjacent bounds checks carefully.'
    );
    expect(suggestion?.split(/\s+/u).length).toBeLessThanOrEqual(14);
    expect(suggestion).toBe('Continue by validating the parser crash with saved artifacts.');
  });

  it('removes dangling conjunctions from model steering suggestions', () => {
    expect(shortSteeringSuggestion('Inspect the saved crash artifacts and.')).toBe(
      'Inspect the saved crash artifacts.'
    );
  });

  it('grounds a generic model suggestion in the latest user steering context', () => {
    const detail = composerDetail('active', {
      run: {
        ...composerDetail('active').run,
        title: 'OAuth callback validation',
        promptMarkdown: 'Review OAuth callback validation.'
      },
      transcriptMessages: [{
        id: 'steering_message',
        runId: 'run_composer',
        attemptId: 'attempt_one',
        traceEventId: 'trace_steering',
        role: 'user',
        phase: 'commentary',
        contentMarkdown: 'Investigate malformed state parameters bypassing OAuth callback validation.',
        source: 'user',
        metadata: {
          nextPromptSuggestions: [{
            title: 'Continue research',
            promptMarkdown: 'Continue from the latest findings.'
          }]
        },
        createdAt: '2026-08-15T10:00:00.000Z'
      }]
    });

    expect(steeringInputSuggestion(detail)).toBe(
      'Continue investigating malformed state parameters bypassing OAuth callback validation.'
    );
  });

  it('uses the session title when no model or steering suggestion is available', () => {
    const detail = composerDetail('paused', {
      run: {
        ...composerDetail('paused').run,
        title: 'Parser bounds-check bypass',
        promptMarkdown: 'Investigate the parser.'
      }
    });

    expect(steeringInputSuggestion(detail)).toBe(
      'Continue investigating Parser bounds-check bypass.'
    );
  });

  it('uses the completed session summary before its original objective', () => {
    const detail = composerDetail('completed', {
      run: {
        ...composerDetail('completed').run,
        title: 'Parser review',
        promptMarkdown: 'Review the request parser for memory-safety issues.',
        summary: 'The investigation confirmed that crafted length fields bypass the parser signed bounds check.'
      }
    });

    expect(steeringInputSuggestion(detail)).toBe(
      'Continue investigating crafted length fields bypass the parser signed bounds check.'
    );
  });

  it('combines model and effort into one model settings picker', () => {
    const html = renderTraceComposer('stopped');
    const providerIconIndex = html.indexOf('class="model-selection-picker-provider-icon"');
    const modelNameIndex = html.indexOf('class="model-selection-picker-model"');

    expect(html).toContain('aria-label="Model settings for the next agent turn"');
    expect(providerIconIndex).toBeGreaterThanOrEqual(0);
    expect(modelNameIndex).toBeGreaterThan(providerIconIndex);
    expect(html).toContain('class="model-selection-picker-model">5.6 Sol</span>');
    expect(html).toContain('class="model-selection-picker-effort">Medium</span>');
    expect(html).not.toContain('aria-label="Model for the next agent turn"');
    expect(html).not.toContain('aria-label="Reasoning effort for the next agent turn"');
  });

  it('orders model, collaboration, and safety controls at the start of the action row', () => {
    const html = renderTraceComposer('stopped', {
      run: {
        ...composerDetail('stopped').run,
        budget: {
          collaboration: {
            mode: 'adaptive',
            subagentMode: 'advanced',
            intensity: 'deep',
            providers: [
              { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'high', enabled: true },
              { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true }
            ]
          }
        }
      }
    });
    const modelIndex = html.indexOf('aria-label="Model settings for the next agent turn"');
    const collaborationIndex = html.indexOf('aria-label="Collaboration settings"');
    const safetyIndex = html.indexOf('aria-label="Shell safety mode"');

    expect(modelIndex).toBeGreaterThanOrEqual(0);
    expect(collaborationIndex).toBeGreaterThanOrEqual(0);
    expect(safetyIndex).toBeGreaterThanOrEqual(0);
    expect(collaborationIndex).toBeGreaterThan(modelIndex);
    expect(safetyIndex).toBeGreaterThan(collaborationIndex);
    expect(html).toContain('main-steer-collaboration-icon');
    expect(html).toContain('class="main-steer-collaboration-label">2 Collabs</span>');
    expect(html).toContain('class="main-steer-collaboration-mode">Advanced</span>');
    expect(html).not.toContain('role="dialog"');
  });

  it('shows context usage immediately before the send control', () => {
    const html = renderTraceComposer('stopped', {
      traceEvents: [traceEvent({
        payload: {
          contextWindow: 200_000,
          usage: { input_tokens: 50_000 }
        }
      })]
    });
    const contextIndex = html.indexOf('aria-label="Context usage: 25%"');
    const sendIndex = html.indexOf('aria-label="Send steering instruction"');

    expect(contextIndex).toBeGreaterThanOrEqual(0);
    expect(sendIndex).toBeGreaterThan(contextIndex);
    expect(html).toContain('class="main-steer-context-usage-value"');
    expect(html).toContain('stroke-dasharray="25 75"');
  });

  it('labels an empty collaboration roster with a zero count', () => {
    const html = renderTraceComposer('stopped');

    expect(html).toContain('class="main-steer-collaboration-label">0 Collabs</span>');
    expect(html).toContain('class="main-steer-collaboration-mode">Simple</span>');
  });

  it('uses the muted mode position for a collaborator matching the lead model', () => {
    const html = renderTraceComposer('stopped', {
      run: {
        ...composerDetail('stopped').run,
        budget: {
          collaboration: {
            mode: 'always',
            subagentMode: 'simple',
            intensity: 'balanced',
            providers: [
              { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'high', enabled: true }
            ]
          }
        }
      }
    });

    expect(html).toContain('class="main-steer-collaboration-mode">Self-Collab</span>');
    expect(html).not.toContain('class="main-steer-collaboration-label"');
  });

  it('uses compact collaborator wording for one non-lead model', () => {
    const html = renderTraceComposer('stopped', {
      run: {
        ...composerDetail('stopped').run,
        budget: {
          collaboration: {
            mode: 'always',
            subagentMode: 'simple',
            intensity: 'balanced',
            providers: [
              { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true }
            ]
          }
        }
      }
    });

    expect(html).toContain('class="main-steer-collaboration-label">1 Collabs</span>');
    expect(html).toContain('class="main-steer-collaboration-mode">Simple</span>');
  });

  it('renders settings-form collaborator rows in Simple mode', () => {
    const html = renderToStaticMarkup(createElement(CollaborationSettingsForm, {
      collaboration: {
        mode: 'always',
        subagentMode: 'simple',
        intensity: 'balanced',
        providers: [
          { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'high', enabled: true },
          { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true }
        ],
        independentFirstPass: false,
        peerChallengeRounds: 0,
        maxConcurrentRooms: 2,
        maxMembersPerRoom: 3
      },
      disabled: false,
      leadModelSelection: { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'medium' },
      providerModelCatalog: providerModelCatalogWithAnthropic(),
      onChange: () => undefined
    }));

    expect(html).toContain('class="settings-form collaboration-selector-form"');
    expect(html).toContain('aria-label="Subagent mode"');
    expect(html.match(/class="settings-form-control-row collaboration-selector-collaborator-row"/g)).toHaveLength(2);
    expect(html.match(/collaboration-selector-model-picker/g)).toHaveLength(2);
    expect(html.match(/class="collaboration-selector-remove"/g)).toHaveLength(2);
    expect(html).not.toContain('collaboration-selector-role');
    expect(html).toContain('class="settings-form-control-row collaboration-selector-add-row"');
  });

  it('adds an inline role dropdown for every Advanced collaborator', () => {
    const html = renderToStaticMarkup(createElement(CollaborationSettingsForm, {
      collaboration: {
        mode: 'always',
        subagentMode: 'advanced',
        intensity: 'balanced',
        providers: [
          { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'high', enabled: true, roles: ['discoverer', 'prover', 'reviewer', 'reporter'] },
          { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true, roles: ['reviewer'] }
        ],
        independentFirstPass: false,
        peerChallengeRounds: 0,
        maxConcurrentRooms: 2,
        maxMembersPerRoom: 3
      },
      disabled: false,
      leadModelSelection: { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'medium' },
      providerModelCatalog: providerModelCatalogWithAnthropic(),
      onChange: () => undefined
    }));

    expect(html.match(/class="collaboration-selector-role-picker"/g)).toHaveLength(2);
    expect(html).toContain('<span>All Roles</span>');
    expect(html).toContain('<span>Reviewer</span>');
    expect(html).toMatch(/type="checkbox" disabled="" checked=""[^>]*><span>Reviewer<\/span>/u);
  });

  it('does not offer removal for the final collaborator', () => {
    const html = renderToStaticMarkup(createElement(CollaborationSettingsForm, {
      collaboration: {
        mode: 'always',
        subagentMode: 'simple',
        intensity: 'balanced',
        providers: [
          { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'high', enabled: true }
        ],
        independentFirstPass: false,
        peerChallengeRounds: 0,
        maxConcurrentRooms: 2,
        maxMembersPerRoom: 3
      },
      disabled: false,
      leadModelSelection: { provider: 'openai-codex', model: 'gpt-5.6-sol', reasoningEffort: 'medium' },
      providerModelCatalog: providerModelCatalogWithAnthropic(),
      onChange: () => undefined
    }));

    expect(html).not.toContain('class="collaboration-selector-remove"');
  });

  it('adds a preferred large model from an unused collaborator provider first', () => {
    const baseModel = providerModelCatalog()[0]!.models[0]!;
    const catalogs: ResearchProviderModelCatalog[] = [
      {
        providerId: 'openai-codex',
        providerName: 'OpenAI',
        models: [baseModel, { ...baseModel, id: 'gpt-5.6-luna', name: 'GPT-5.6 Luna' }]
      },
      {
        providerId: 'anthropic',
        providerName: 'Anthropic',
        models: [
          { ...baseModel, id: 'claude-haiku-5', name: 'Claude Haiku 5' },
          { ...baseModel, id: 'claude-opus-5', name: 'Claude Opus 5' }
        ]
      },
      {
        providerId: 'xai',
        providerName: 'xAI',
        models: [
          { ...baseModel, id: 'grok-mini', name: 'Grok Mini' },
          { ...baseModel, id: 'grok-4.6', name: 'Grok 4.6' }
        ]
      }
    ];
    const collaboration = {
      mode: 'always' as const,
      subagentMode: 'simple' as const,
      intensity: 'balanced' as const,
      providers: [
        { provider: 'openai-codex' as const, model: baseModel.id, reasoningEffort: 'high' as const, enabled: true }
      ],
      independentFirstPass: false,
      peerChallengeRounds: 0,
      maxConcurrentRooms: 2,
      maxMembersPerRoom: 3
    };
    const defaults = {
      anthropic: { largeModel: 'claude-opus-5', smallModel: 'claude-haiku-5', reasoningEffort: 'high' as const },
      xai: { largeModel: 'grok-4.6', smallModel: 'grok-mini', reasoningEffort: 'high' as const }
    };

    expect(selectNextCollaborationRoute(collaboration, catalogs, 'openai-codex', defaults)).toMatchObject({
      provider: 'anthropic',
      model: { id: 'claude-opus-5' }
    });
    expect(selectNextCollaborationRoute({
      ...collaboration,
      providers: [
        ...collaboration.providers,
        { provider: 'anthropic', model: 'claude-opus-5', reasoningEffort: 'high', enabled: true }
      ]
    }, catalogs, 'openai-codex', defaults)).toMatchObject({
      provider: 'xai',
      model: { id: 'grok-4.6' }
    });
  });

  it('places the persisted shell safety picker after model settings', () => {
    const html = renderTraceComposer('stopped');
    const safetyIndex = html.indexOf('aria-label="Shell safety mode"');
    const shieldIndex = html.indexOf('main-steer-safety-mode-icon', safetyIndex);
    const modelIndex = html.indexOf('aria-label="Model settings for the next agent turn"');

    expect(html).toContain('Auto-Review');
    expect(safetyIndex).toBeGreaterThanOrEqual(0);
    expect(shieldIndex).toBeGreaterThan(safetyIndex);
    expect(safetyIndex).toBeGreaterThan(modelIndex);
    expect(SHELL_SAFETY_MODE_OPTIONS).toEqual([
      { value: 'manual_approval', label: 'Manual Approval' },
      { value: 'auto_review', label: 'Auto-Review' },
      { value: 'danger', label: 'Danger Mode' }
    ]);
    expect(steeringSafetyModeOptions(SHELL_SAFETY_MODE_OPTIONS, false).map((option) => option.value)).toEqual([
      'manual_approval',
      'auto_review'
    ]);
    expect(steeringSafetyModeOptions(SHELL_SAFETY_MODE_OPTIONS, true).map((option) => option.value)).toEqual([
      'manual_approval',
      'auto_review',
      'danger'
    ]);
  });

  it('replaces the steering composer with an inline Auto-Review override question', () => {
    const html = renderToStaticMarkup(createElement(MainSteerArea, {
      busy: false,
      detail: composerDetail('active'),
      providerModelCatalog: providerModelCatalog(),
      runId: 'run_composer',
      shellApproval: autoReviewOverrideApproval(),
      onShellApprovalDecision: () => undefined,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).toContain('aria-label="Approve shell command once"');
    expect(html).toContain('>Approve Once</button>');
    expect(html).not.toContain('aria-label="Steer research session"');
    expect(html).not.toContain('aria-label="Stop session"');
  });

  it('does not render trace filters', () => {
    const html = renderToStaticMarkup(createElement(MainSteerArea, {
      busy: false,
      detail: composerDetail('stopped'),
      providerModelCatalog: providerModelCatalog(),
      runId: 'run_composer',
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).not.toContain('aria-label="Trace filters');
    expect(html).toContain('class="main-steer-input-row without-trace-filters"');
  });

  it('can omit collaboration without leaving an empty action-row slot', () => {
    const html = renderToStaticMarkup(createElement(MainSteerArea, {
      busy: false,
      detail: composerDetail('stopped'),
      providerModelCatalog: providerModelCatalog(),
      runId: 'run_composer',
      showCollaboration: false,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).toContain('class="main-steer-input-row without-trace-filters without-collaboration"');
    expect(html).not.toContain('aria-label="Collaboration settings"');
    expect(renderTraceComposer('stopped')).toContain('aria-label="Collaboration settings"');
  });

  it('can omit collaboration and safety mode without leaving empty action-row slots', () => {
    const html = renderToStaticMarkup(createElement(MainSteerArea, {
      busy: false,
      detail: composerDetail('stopped'),
      providerModelCatalog: providerModelCatalog(),
      runId: 'run_composer',
      showCollaboration: false,
      showSafetyMode: false,
      onSessionAction: () => undefined,
      onSteerInstruction: () => undefined
    }));

    expect(html).toContain('class="main-steer-input-row without-trace-filters without-collaboration without-safety-mode"');
    expect(html).not.toContain('aria-label="Collaboration settings"');
    expect(html).not.toContain('aria-label="Shell safety mode"');
    expect(renderTraceComposer('stopped')).toContain('aria-label="Shell safety mode"');
  });
});

function renderTraceComposer(status: RunStatus, detailPatch: Partial<RunDetail> = {}): string {
  return renderToStaticMarkup(createElement(MainSteerArea, {
    busy: false,
    detail: composerDetail(status, detailPatch),
    providerModelCatalog: providerModelCatalog(),
    runId: 'run_composer',
    onSessionAction: () => undefined,
    onSteerInstruction: () => undefined
  }));
}

function composerDetail(status: RunStatus, detailPatch: Partial<RunDetail> = {}): RunDetail {
  return {
    run: {
      id: 'run_composer',
      status,
      shellSafetyMode: 'auto_review',
      model: 'gpt-5.6-sol',
      reasoningEffort: 'medium',
      budget: {}
    },
    attempts: [],
    traceEvents: [],
    transcriptMessages: [],
    modelSessions: [],
    contextCompactions: [],
    ...detailPatch
  } as unknown as RunDetail;
}

function traceEvent(input: Partial<TraceEventRecord> = {}): TraceEventRecord {
  return {
    id: 'trace_composer',
    runId: 'run_composer',
    attemptId: null,
    sequence: 1,
    source: 'model',
    type: 'model_message',
    summary: 'Response completed.',
    payload: {},
    sensitivity: 'internal',
    modelVisible: true,
    createdAt: '2026-08-27T00:00:00.000Z',
    artifactId: null,
    toolCallId: null,
    approvalId: null,
    ...input
  };
}

function autoReviewOverrideApproval(): ApprovalRecord {
  return {
    id: 'approval_auto_review_override',
    runId: 'run_composer',
    attemptId: 'attempt_one',
    requestKind: 'shell_command',
    requestedAction: {
      approvalKind: 'auto_review_override',
      mode: 'auto_review',
      reviewReason: 'The proof command needs researcher confirmation.'
    },
    decision: 'pending',
    reason: 'Waiting for the researcher to approve this Auto-Review denial once.',
    scopeAmendmentId: null,
    createdAt: '2026-08-15T00:00:00.000Z',
    decidedAt: null
  };
}

function providerModelCatalog(): ResearchProviderModelCatalog[] {
  return [{
      providerId: 'openai-codex',
      providerName: 'OpenAI',
      models: [{
        id: 'gpt-5.6-sol',
        name: 'GPT-5.6 Sol',
        reasoning: true,
        effortLevels: ['low', 'medium', 'high'],
        contextWindow: 400_000,
        maxTokens: 128_000
      }]
    }];
}

function providerModelCatalogWithAnthropic(): ResearchProviderModelCatalog[] {
  return [
    ...providerModelCatalog(),
    {
      providerId: 'anthropic',
      providerName: 'Anthropic',
      models: [{
        id: 'claude-opus-5',
        name: 'Claude Opus 5',
        reasoning: true,
        effortLevels: ['low', 'medium', 'high'],
        contextWindow: 400_000,
        maxTokens: 128_000
      }]
    }
  ];
}
