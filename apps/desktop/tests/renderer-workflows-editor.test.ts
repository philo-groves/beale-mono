import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { SessionWorkflowDefinition, SessionWorkflowNotebook, WorkspaceRegistryEntry } from '../src/shared/types';
import { WorkflowFieldDialog, WorkflowsWorkspace, resolveWorkflowSessionStatus, workflowCellRunStatus, workflowRunStatusLabel } from '../src/renderer/features/workflows/WorkflowsWorkspace';
import { workflowRunInput } from '../src/renderer/view-models/runSettings';
import { normalizeResearchCollaboration } from '../src/shared/collaboration';

const notebook: SessionWorkflowNotebook = { nbformat: 4, nbformat_minor: 5, metadata: { beale: { kind: 'session_workflow' } }, cells: [
  { id: 'inspect', cell_type: 'markdown', metadata: { beale: { title: 'Inspect source' } }, source: ['Read the example source.'] },
  { id: 'report', cell_type: 'code', metadata: { beale: { title: 'Report', language: 'python3' }, vscode: { languageId: 'python3' } },
    source: ['print("example")'], execution_count: null, outputs: [] }
] };

const workflow: SessionWorkflowDefinition = {
  id: 'workflow-example',
  title: 'Example review',
  description: 'Review an example component.',
  kind: 'custom',
  revision: 2,
  fields: [{ id: 'component', label: 'Component', description: 'Choose the component to review.', placeholder: 'Example component', required: true, multiline: false }],
  notebook,
  steps: [
    { id: 'inspect', title: 'Inspect source', instructions: 'Read the example source.' },
    { id: 'report', title: 'Report', instructions: 'Summarize the example.' }
  ]
};
const workspaces = [{ id: 'registered-example', workspaceId: 'workspace-example', workspaceName: 'Example workspace' }] as WorkspaceRegistryEntry[];

describe('Workflows editor', () => {
  it('opens a new session with this workflow assigned and editable operator values', () => {
    const input = workflowRunInput(workflow, 'auto_review', { component: 'example module' },
      { defaultProviderId: 'openai-codex', modelDefaults: { 'openai-codex': {
        largeModel: 'example-large-model', smallModel: 'example-small-model', reasoningEffort: 'high'
      } } });
    expect(input.guidanceWorkflow).toEqual({ id: workflow.id, values: { component: 'example module' } });
    expect(input.promptMarkdown).toContain('Run the assigned Example review workflow');
    expect(input.promptMarkdown).toContain('Component: example module');
    expect(input.shellSafetyMode).toBe('auto_review');
    expect(input.provider).toBe('openai-codex');
    expect(input.model).toBe('example-large-model');
    expect(input.reasoningEffort).toBe('high');
    expect(input.collaboration).toMatchObject({ mode: 'always', subagentMode: 'advanced',
      providers: [{ provider: 'openai-codex', model: 'example-large-model', reasoningEffort: 'high', enabled: true }] });
    expect(input.collaboration?.providers).toHaveLength(1);
    expect(normalizeResearchCollaboration(input.collaboration).providers).toHaveLength(1);
  });

  it('rejects a model without a supported collaborator reasoning effort', () => {
    expect(() => workflowRunInput(workflow, 'auto_review', {},
      { defaultProviderId: 'openai-codex', modelDefaults: { 'openai-codex': {
        largeModel: 'example-large-model', smallModel: 'example-small-model', reasoningEffort: 'off'
      } } })).toThrow(/reasoning effort/u);
    expect(() => workflowRunInput(workflow, 'auto_review', {},
      { defaultProviderId: 'openai-codex', modelDefaults: {} })).toThrow(/default Lead provider and large model/u);
  });

  it('asks for a workspace when none is selected', () => {
    const html = renderToStaticMarkup(createElement(WorkflowsWorkspace, {
      workflows: [workflow], workspaces, selectedWorkspaceId: null, workspaceReady: false,
      selectedId: workflow.id, creating: false, busy: false, error: null,
      onScopeChange: () => undefined, onRunAll: () => undefined, onCreate: async () => null, onUpdate: async () => null
    }));
    expect(html).toContain('Example workspace');
    expect(html).toContain('Select a workspace first');
    expect(html).not.toContain('Workflow name');
  });

  it('shows the workflow name and description with inline edit controls above ordered cells', () => {
    const html = renderToStaticMarkup(createElement(WorkflowsWorkspace, {
      workflows: [workflow], workspaces, selectedWorkspaceId: workspaces[0]!.workspaceId, workspaceReady: true,
      selectedId: workflow.id, creating: false, busy: false, error: null,
      onScopeChange: () => undefined, onRunAll: () => undefined, onCreate: async () => null, onUpdate: async () => null
    }));
    expect(html.indexOf('Example review')).toBeLessThan(html.indexOf('Cell 1'));
    expect(html).toContain('Review an example component.');
    expect(html).toContain('aria-label="Edit workflow name"');
    expect(html).toContain('aria-label="Edit workflow description"');
    expect(html).not.toContain('Built-in workflow · Revision');
    expect(html).not.toContain('Configure the session form');
    expect(html).not.toContain('aria-label="Workflow name"');
    expect(html).toContain('aria-selected="true"');
    expect(html).not.toContain('<h2>Setup form</h2>');
    expect(html).not.toContain('<h2>Runbook cells</h2>');
    expect(html).not.toContain('Session text fields');
    expect(html).toContain('Run All');
    expect(html).toContain('aria-label="Workflow run history"');
    expect(html).toContain('Current');
    expect(html).toContain('Not started');
    expect(html).toContain('+ Add a field');
    expect(html).toContain('Choose the component to review.');
    expect(html).toContain('placeholder="Example component"');
    expect(html).toContain('aria-label="Configure Component"');
    expect(html.match(/class="settings-form-control-row workflow-field-preview-row"/g)).toHaveLength(1);
    expect(html).not.toContain('Field title');
    expect(html).not.toContain('Input size');
    expect(html).toContain('workflow-fields-toolbar');
    expect(html).toContain('settings-form-squircle workflow-fields-form');
    expect(html).toContain('Cell 1');
    expect(html).toContain('Cell 2');
    expect(html).toContain('Markdown');
    expect(html).toContain('Python');
    expect(html).toContain('print(&quot;example&quot;)');
    expect(html).toContain('Move down');
    expect(html).toContain('Add markdown');
    expect(html).toContain('Add code');
    expect(html).toContain('Save changes');
  });

  it('labels completed, active, and pending cells from one run assignment', () => {
    const assignment = { definition: workflow, stepIndex: 1, completedAt: null, engagedAt: '2026-01-01T00:00:00.000Z' } as NonNullable<Parameters<typeof workflowCellRunStatus>[0]>;
    expect(workflowCellRunStatus(assignment, 'inspect', 0, 'active')).toBe('Completed');
    expect(workflowCellRunStatus(assignment, 'report', 1, 'active')).toBe('Running');
    expect(workflowCellRunStatus(assignment, 'report', 1, 'blocked')).toBe('Ended early');
    expect(workflowCellRunStatus(assignment, 'report', 1, 'stopped')).toBe('Ended early');
    expect(workflowRunStatusLabel(assignment, 'stopped')).toBe('Ended early · stopped');
    expect(workflowRunStatusLabel(assignment, 'active')).toBe('Running');
    const stoppedBeforeFirstCell = { ...assignment, stepIndex: 0 };
    expect(workflowCellRunStatus(stoppedBeforeFirstCell, 'report', 1, 'stopped')).toBe('Not reached');
    expect(resolveWorkflowSessionStatus('active', 'failed')).toBe('failed');
    expect(resolveWorkflowSessionStatus('stopped', 'active')).toBe('stopped');
  });

  it('starts a new workflow with editable name and description inputs', () => {
    const html = renderToStaticMarkup(createElement(WorkflowsWorkspace, {
      workflows: [workflow], workspaces, selectedWorkspaceId: workspaces[0]!.workspaceId, workspaceReady: true,
      selectedId: null, creating: true, busy: false, error: null,
      onScopeChange: () => undefined, onRunAll: () => undefined, onCreate: async () => null, onUpdate: async () => null
    }));
    expect(html).toContain('aria-label="Workflow name"');
    expect(html).toContain('aria-label="Workflow description"');
    expect(html).toContain('aria-label="Finish editing workflow name"');
    expect(html).toContain('aria-label="Finish editing workflow description"');
  });

  it('keeps built-in cell structure fixed while exposing editable text', () => {
    const steps = ['scope', 'inventory', 'map', 'review', 'validate', 'report'].map((id) => ({ id, title: id, instructions: `Review example ${id}.` }));
    const builtIn: SessionWorkflowDefinition = { ...workflow, id: 'beale.repository-auditor', kind: 'repository_auditor', steps,
      notebook: { ...notebook, cells: steps.map((step) => ({ id: step.id, cell_type: 'markdown', metadata: { beale: { title: step.title } }, source: [step.instructions] })) } };
    const html = renderToStaticMarkup(createElement(WorkflowsWorkspace, {
      workflows: [builtIn], workspaces, selectedWorkspaceId: workspaces[0]!.workspaceId, workspaceReady: true,
      selectedId: builtIn.id, creating: false, busy: false, error: null,
      onScopeChange: () => undefined, onRunAll: () => undefined, onCreate: async () => null, onUpdate: async () => null
    }));
    expect(html).toContain('Cell title');
    expect(html).toContain('Markdown');
    expect(html).toContain('keeps its six cell IDs and order');
    expect(html).toContain('Run All');
    expect(html).not.toContain('Move down');
  });

  it('renders multiline fields as one form row and keeps configuration in the dialog', () => {
    const multiline: SessionWorkflowDefinition = { ...workflow, fields: [{ ...workflow.fields[0]!, multiline: true }] };
    const html = renderToStaticMarkup(createElement(WorkflowsWorkspace, {
      workflows: [multiline], workspaces, selectedWorkspaceId: workspaces[0]!.workspaceId, workspaceReady: true,
      selectedId: multiline.id, creating: false, busy: false, error: null,
      onScopeChange: () => undefined, onRunAll: () => undefined, onCreate: async () => null, onUpdate: async () => null
    }));
    expect(html).toContain('<textarea aria-label="Component"');
    expect(html.match(/class="settings-form-control-row workflow-field-preview-row"/g)).toHaveLength(1);
    const dialog = renderToStaticMarkup(createElement(WorkflowFieldDialog, {
      field: multiline.fields[0]!, creating: false, fixedAuditorField: false,
      onClose: () => undefined, onApply: () => undefined, onRemove: () => undefined
    }));
    expect(dialog).toContain('Field title');
    expect(dialog).toContain('Short description');
    expect(dialog).toContain('Placeholder');
    expect(dialog).toContain('One line');
    expect(dialog).toContain('Multiple lines');
    expect(dialog).toContain('Required at session start');
    expect(dialog).toContain('Remove field');
  });

  it('keeps the auditor core field requirement fixed in its dialog', () => {
    const dialog = renderToStaticMarkup(createElement(WorkflowFieldDialog, {
      field: { id: 'systems', label: 'Systems', required: true, multiline: true },
      creating: false, fixedAuditorField: true,
      onClose: () => undefined, onApply: () => undefined, onRemove: () => undefined
    }));
    expect(dialog).toContain('Input size');
    expect(dialog).toContain('Required at session start');
    expect(dialog).toContain('disabled=""');
    expect(dialog).not.toContain('Remove field');
  });
});
