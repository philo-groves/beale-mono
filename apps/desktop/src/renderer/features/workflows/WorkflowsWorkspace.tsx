import { useEffect, useState } from 'react';
import type { FormEvent, JSX, PointerEvent as ReactPointerEvent } from 'react';
import { Check, Pencil, Plus } from 'lucide-react';
import type { ResearchSessionSummary, RunStatus, SessionWorkflowAssignment, SessionWorkflowDefinition, SessionWorkflowDraft, SessionWorkflowField, SessionWorkflowNotebook, SessionWorkflowNotebookCell, SessionWorkflowRunSummary, SessionWorkflowUpdateInput, WorkspaceRegistryEntry } from '@shared/types';
import { CollectionSidebar } from '../../app/CollectionSidebar';
import { Modal } from '../../app/Modal';
import { renderHighlightedCodeBlock, renderTraceProseText } from '../traces/traceMarkup';

const CODE_LANGUAGES = [
  ['python3', 'Python'], ['sh', 'Shell'], ['bash', 'Bash'], ['zsh', 'Zsh'],
  ['javascript', 'JavaScript'], ['ruby', 'Ruby'], ['perl', 'Perl'], ['pwsh', 'PowerShell']
] as const;

function newNotebookCell(kind: 'markdown' | 'code'): SessionWorkflowNotebookCell {
  const id = newCellId();
  return kind === 'code'
    ? { id, cell_type: 'code', metadata: { beale: { title: '', language: 'python3' }, vscode: { languageId: 'python3' } }, source: [''], execution_count: null, outputs: [] }
    : { id, cell_type: 'markdown', metadata: { beale: { title: '' } }, source: [''] };
}

function newNotebook(): SessionWorkflowNotebook {
  return { nbformat: 4, nbformat_minor: 5, metadata: { beale: { kind: 'session_workflow' } }, cells: [newNotebookCell('markdown')] };
}

function notebookSourceLines(source: string): string[] { return source.match(/[^\n]*\n|[^\n]+$/gu) ?? ['']; }

function terminalSessionStatus(status: RunStatus | null | undefined): boolean {
  return status === 'blocked' || status === 'completed' || status === 'failed' || status === 'stopped';
}

export function resolveWorkflowSessionStatus(canonical: RunStatus | null | undefined, local: RunStatus | null | undefined): RunStatus | undefined {
  if (terminalSessionStatus(canonical)) return canonical ?? undefined;
  if (terminalSessionStatus(local)) return local ?? undefined;
  return canonical ?? local ?? undefined;
}

export function workflowCellRunStatus(assignment: SessionWorkflowAssignment | null, cellId: string, _index: number, sessionStatus?: RunStatus): string {
  if (!assignment) return 'Not started';
  const runIndex = assignment.definition.notebook.cells.findIndex((cell) => cell.id === cellId);
  if (runIndex < 0) return 'Not in run';
  if (runIndex < assignment.stepIndex) return 'Completed';
  const endedEarly = !assignment.completedAt && terminalSessionStatus(sessionStatus);
  if (runIndex > assignment.stepIndex) return endedEarly ? 'Not reached' : 'Pending';
  if (assignment.completedAt) return 'Completed';
  if (endedEarly) return 'Ended early';
  if (sessionStatus === 'queued') return 'Queued';
  if (sessionStatus === 'paused') return 'Paused';
  return assignment.engagedAt ? 'Running' : 'Queued';
}

export function workflowRunStatusLabel(assignment: SessionWorkflowAssignment, sessionStatus?: RunStatus): string {
  if (assignment.completedAt) return 'Workflow completed';
  if (terminalSessionStatus(sessionStatus)) {
    return `Ended early · ${sessionStatus}`;
  }
  if (sessionStatus === 'paused') return 'Paused';
  return assignment.engagedAt ? 'Running' : 'Queued';
}

export function WorkflowsSidebar({ workflows, selectedId, collapsed, canCreate, error, onSelect, onCreate, onResizePointerDown }: {
  workflows: SessionWorkflowDefinition[];
  selectedId: string | null;
  collapsed: boolean;
  canCreate: boolean;
  error: string | null;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onResizePointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
}): JSX.Element {
  return <CollectionSidebar title="Workflows" label="Workflows sidebar" collapsed={collapsed} error={error}
    updateKey={workflows.map((workflow) => workflow.id).join(',')} onResizePointerDown={onResizePointerDown}
    primaryAction={<button type="button" className="sidebar-utility-button" title="Create workflow" aria-label="Create workflow" disabled={!canCreate} onClick={onCreate}><Plus size={17} /></button>}>
    <nav className="collection-sidebar-list sidebar-list-scroll-content" aria-label="Session workflows">
      <div className="collection-sidebar-list-heading">Available</div>
      {workflows.length === 0 && !error ? <p className="collection-sidebar-empty">{canCreate ? 'Loading workflows…' : 'Select a workspace to manage workflows.'}</p> : null}
      {workflows.map((workflow) => <div className={`workspace-item-row no-menu ${selectedId === workflow.id ? 'active' : ''}`} key={workflow.id}>
        <button type="button" className="workspace-item collection-sidebar-item" aria-current={selectedId === workflow.id ? 'page' : undefined} onClick={() => onSelect(workflow.id)}>
          <span className="collection-sidebar-item-copy"><span>{workflow.title}</span><small>{workflow.kind === 'repository_auditor' ? 'Built in' : 'Custom'}</small></span>
        </button>
      </div>)}
    </nav>
  </CollectionSidebar>;
}

export function WorkflowsWorkspace({ workflows, workspaces, selectedWorkspaceId, workspaceReady, selectedId, creating, busy, error, sessions, currentValues, onCurrentValuesChange, onOpenSession, onScopeChange, onRunAll, onCreate, onUpdate }: {
  workflows: SessionWorkflowDefinition[];
  workspaces: readonly WorkspaceRegistryEntry[];
  selectedWorkspaceId: string | null;
  workspaceReady: boolean;
  selectedId: string | null;
  creating: boolean;
  busy: boolean;
  error: string | null;
  sessions?: readonly ResearchSessionSummary[];
  currentValues?: Record<string, string> | null;
  onCurrentValuesChange?: (values: Record<string, string>) => void;
  onOpenSession?: (sessionId: string) => void;
  onScopeChange: (workspaceId: string) => void;
  onRunAll: (workflow: SessionWorkflowDefinition, values: Record<string, string>) => Promise<string | null> | void;
  onCreate: (input: SessionWorkflowDraft) => Promise<SessionWorkflowDefinition | null>;
  onUpdate: (input: SessionWorkflowUpdateInput) => Promise<SessionWorkflowDefinition | null>;
}): JSX.Element {
  const selected = workflows.find((workflow) => workflow.id === selectedId) ?? workflows[0];
  const [title, setTitle] = useState(creating ? '' : selected?.title ?? '');
  const [description, setDescription] = useState(creating ? '' : selected?.description ?? '');
  const [editingTitle, setEditingTitle] = useState(creating);
  const [editingDescription, setEditingDescription] = useState(creating);
  const [fields, setFields] = useState<SessionWorkflowField[]>(creating ? [] : selected?.fields ?? []);
  const [localFieldValues, setLocalFieldValues] = useState<Record<string, string>>({});
  const [runSelection, setRunSelection] = useState('current');
  const [runs, setRuns] = useState<SessionWorkflowRunSummary[]>([]);
  const [runAssignment, setRunAssignment] = useState<SessionWorkflowAssignment | null>(null);
  const [runHistoryError, setRunHistoryError] = useState<string | null>(null);
  const [runRefreshVersion, setRunRefreshVersion] = useState(0);
  const [fieldDialog, setFieldDialog] = useState<{ index: number | null; field: SessionWorkflowField } | null>(null);
  const [notebook, setNotebook] = useState<SessionWorkflowNotebook>(creating ? newNotebook() : selected?.notebook ?? newNotebook());
  const [revision, setRevision] = useState(selected?.revision ?? 0);
  useEffect(() => {
    if (!workspaceReady || !selected || creating) return;
    let cancelled = false;
    const refresh = async (): Promise<void> => {
      try {
        const nextRuns = await window.beale.listSessionWorkflowRuns(selected.id);
        const sessionId = runSelection === 'current' ? nextRuns[0]?.sessionId : runSelection;
        const assignment = sessionId ? await window.beale.getSessionWorkflow(sessionId) : null;
        if (!cancelled) { setRuns(nextRuns); setRunAssignment(assignment); setRunHistoryError(null); }
      } catch (caught) {
        if (!cancelled) setRunHistoryError(caught instanceof Error ? caught.message : String(caught));
      }
    };
    void refresh();
    const timer = window.setInterval(() => { void refresh(); }, 10_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [workspaceReady, selected?.id, creating, runSelection, runRefreshVersion]);
  const historyMode = runSelection !== 'current';
  const currentSessionId = runSelection === 'current' ? runs[0]?.sessionId : runSelection;
  const displayedAssignment = runAssignment?.sessionId === currentSessionId ? runAssignment : null;
  const viewDefinition = historyMode ? displayedAssignment?.definition ?? selected : selected;
  const viewFields = historyMode ? viewDefinition?.fields ?? [] : fields;
  const viewNotebook = historyMode ? viewDefinition?.notebook ?? notebook : notebook;
  const fieldValues = historyMode ? displayedAssignment?.values ?? {} : currentValues ?? (Object.keys(localFieldValues).length ? localFieldValues : displayedAssignment?.values ?? {});
  const changeFieldValues = (values: Record<string, string>): void => {
    setLocalFieldValues(values);
    onCurrentValuesChange?.(values);
  };
  const currentRun = runs.find((run) => run.sessionId === currentSessionId);
  const registrySession = sessions?.find((session) => session.runId === currentSessionId);
  const sessionStatus = resolveWorkflowSessionStatus(currentRun?.sessionStatus, registrySession?.status);
  const sessionEndedAt = currentRun?.sessionEndedAt ?? registrySession?.endedAt;
  const statusForRun = (run: SessionWorkflowRunSummary): RunStatus | undefined =>
    resolveWorkflowSessionStatus(run.sessionStatus, sessions?.find((session) => session.runId === run.sessionId)?.status);
  const builtIn = !creating && selected?.kind === 'repository_auditor';
  const draft = { title, description, fields, notebook };
  const dirty = creating || JSON.stringify(draft) !== JSON.stringify({ title: selected?.title, description: selected?.description, fields: selected?.fields, notebook: selected?.notebook });
  const reset = (): void => {
    setTitle(creating ? '' : selected?.title ?? '');
    setDescription(creating ? '' : selected?.description ?? '');
    setEditingTitle(creating);
    setEditingDescription(creating);
    setFields(creating ? [] : selected?.fields ?? []);
    setFieldDialog(null);
    setNotebook(creating ? newNotebook() : selected?.notebook ?? newNotebook());
    setRevision(selected?.revision ?? 0);
  };
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (historyMode) return;
    if (!title.trim()) { setEditingTitle(true); return; }
    if (!description.trim()) { setEditingDescription(true); return; }
    void (async () => {
      const saved = creating ? await onCreate(draft) : selected ? await onUpdate({ ...draft, id: selected.id, expectedRevision: revision }) : null;
      if (!saved) return;
      if (creating) return;
      setTitle(saved.title);
      setDescription(saved.description);
      setFields(saved.fields);
      setNotebook(saved.notebook);
      setRevision(saved.revision);
      setEditingTitle(false);
      setEditingDescription(false);
    })();
  };
  const applyField = (field: SessionWorkflowField): void => {
    if (!fieldDialog) return;
    setFields((current) => fieldDialog.index === null ? [...current, field]
      : current.map((existing, index) => index === fieldDialog.index ? field : existing));
    setFieldDialog(null);
  };
  const removeField = (): void => {
    if (fieldDialog?.index === null || fieldDialog?.index === undefined) return;
    const fieldId = fieldDialog.field.id;
    setFields((current) => current.filter((_, index) => index !== fieldDialog.index));
    const next = { ...fieldValues }; delete next[fieldId]; changeFieldValues(next);
    setFieldDialog(null);
  };
  const updateCell = (index: number, change: Partial<SessionWorkflowNotebookCell>): void => setNotebook((current) => ({ ...current, cells: current.cells.map((cell, position) => position === index ? { ...cell, ...change } : cell) }));
  const moveCell = (index: number, offset: number): void => setNotebook((current) => {
    const updated = [...current.cells];
    [updated[index], updated[index + offset]] = [updated[index + offset]!, updated[index]!];
    return { ...current, cells: updated };
  });
  return <main className="workflow-workspace">
    <div className="workflow-workspace-tabs research-side-view-tabs research-side-view-tabs-scrollable pill-view-tabs" role="tablist" aria-label="Workflow workspace scope">
      {workspaces.filter((workspace) => workspace.workspaceId.length > 0).map((workspace) => {
        const active = selectedWorkspaceId === workspace.workspaceId;
        return <div className={`research-side-view-tab provider-settings-tab ${active ? 'active' : ''}`.trim()} key={workspace.id}>
          <button type="button" className="research-side-view-tab-activate" role="tab" aria-selected={active} aria-controls="workflow-workspace-panel" disabled={busy}
            onClick={() => onScopeChange(workspace.workspaceId)}><span>{workspace.workspaceName}</span></button>
        </div>;
      })}
    </div>
    <div id="workflow-workspace-panel" role="tabpanel">
    {!selectedWorkspaceId ? <div className="workflow-empty"><h1>Workflows</h1><p>Select a workspace first to view and edit workflows.</p></div>
      : !workspaceReady ? <div className="workflow-empty"><h1>Workflows</h1><p>{error ? 'Could not open the workspace. Select it again to retry.' : 'Opening workspace…'}</p>{error ? <p role="alert" className="error-box">{error}</p> : null}</div>
      : !creating && !selected ? <div className="workflow-empty"><h1>Workflows</h1><p>{error ? 'Unable to load workflows.' : 'Loading workflows…'}</p>{error ? <p role="alert" className="error-box">{error}</p> : null}</div>
      : <><form className="workflow-editor" onSubmit={submit}>
    <header className="workflow-editor-heading">
      <div className="workflow-editor-heading-copy">
        <div className="workflow-heading-line workflow-heading-title">
          {!historyMode && editingTitle ? <input autoFocus required aria-label="Workflow name" maxLength={160} value={title} onChange={(event) => setTitle(event.target.value)}
            onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); if (title.trim()) setEditingTitle(false); } }} />
            : <h1>{historyMode ? viewDefinition?.title : title || 'Untitled workflow'}</h1>}
          {!historyMode ? <button type="button" className="workflow-heading-edit-button" aria-label={editingTitle ? 'Finish editing workflow name' : 'Edit workflow name'}
            title={editingTitle ? 'Finish editing workflow name' : 'Edit workflow name'} disabled={busy || (editingTitle && !title.trim())}
            onClick={() => setEditingTitle((current) => !current)}>
            {editingTitle ? <Check size={16} aria-hidden="true" /> : <Pencil size={15} aria-hidden="true" />}
          </button> : null}
        </div>
        <div className="workflow-heading-line workflow-heading-description">
          {!historyMode && editingDescription ? <textarea autoFocus={!editingTitle} required aria-label="Workflow description" maxLength={2000} rows={3} value={description}
            onChange={(event) => setDescription(event.target.value)} />
            : <p>{historyMode ? viewDefinition?.description : description || 'Add a workflow description.'}</p>}
          {!historyMode ? <button type="button" className="workflow-heading-edit-button" aria-label={editingDescription ? 'Finish editing workflow description' : 'Edit workflow description'}
            title={editingDescription ? 'Finish editing workflow description' : 'Edit workflow description'} disabled={busy || (editingDescription && !description.trim())}
            onClick={() => setEditingDescription((current) => !current)}>
            {editingDescription ? <Check size={16} aria-hidden="true" /> : <Pencil size={15} aria-hidden="true" />}
          </button> : null}
        </div>
      </div>
      <div className="workflow-editor-actions">
        {!creating ? <select className="workflow-run-selector" aria-label="Workflow run history" value={runSelection} onChange={(event) => setRunSelection(event.target.value)}>
          <option value="current">Current</option>
          {runs.slice(1).map((run) => <option value={run.sessionId} key={run.sessionId}>
            {run.startedAt ? new Date(run.startedAt).toLocaleString() : 'Earlier run'} · {run.stepIndex}/{run.cellCount} cells{statusForRun(run) ? ` · ${statusForRun(run)}` : ''} · {run.sessionId.slice(0, 8)}
          </option>)}
        </select> : null}
        {!historyMode ? <button type="button" className="secondary-button" disabled={busy || !dirty} onClick={reset}>Reset changes</button> : null}
        {!historyMode ? <button type="submit" className="primary-button" disabled={busy || !dirty}>{busy ? 'Saving…' : creating ? 'Create workflow' : 'Save changes'}</button> : null}
      </div>
    </header>
    {historyMode && !displayedAssignment ? <p className="workflow-empty-note">Loading selected run…</p> : null}
    {currentSessionId && displayedAssignment ? <div className="workflow-run-summary">
      <span>{runSelection === 'current' ? 'Current run' : 'Past run'} · {displayedAssignment.stepIndex}/{displayedAssignment.definition.notebook.cells.length} cells completed · {workflowRunStatusLabel(displayedAssignment, sessionStatus)}{sessionEndedAt ? ` · ${new Date(sessionEndedAt).toLocaleString()}` : ''}</span>
      {onOpenSession ? <button type="button" className="secondary-button" onClick={() => onOpenSession(currentSessionId)}>Open session</button> : null}
    </div> : null}
    {runHistoryError ? <p role="alert" className="error-box">Could not load workflow runs: {runHistoryError}</p> : null}
    <section className="workflow-setup-form" aria-label="Workflow setup form">
      <div className="workflow-fields-toolbar">
        {!historyMode ? <button type="button" className="primary-button" disabled={busy || creating || dirty || !selected}
          title={dirty && !creating ? 'Save changes before starting a run' : undefined}
          onClick={() => { if (selected) void Promise.resolve(onRunAll(selected, fieldValues)).then((sessionId) => {
            if (sessionId) { setRunSelection('current'); setRunRefreshVersion((version) => version + 1); }
          }); }}>Run All</button> : <span>Saved configuration for this run</span>}
        {!historyMode ? <button type="button" className="secondary-button" disabled={busy || fields.length >= 20}
          onClick={() => setFieldDialog({ index: null, field: { id: newFieldId(), label: '', description: '', required: false } })}>+ Add a field</button> : null}
      </div>
      <div className="settings-form-squircle workflow-fields-form">
        {viewFields.length === 0 ? <p className="workflow-empty-note">No session fields. Add a field to configure what the operator enters when this workflow starts.</p> : null}
        <div className="settings-form-control-list">
          {viewFields.map((field, index) => <div className="settings-form-control-row workflow-field-preview-row" key={field.id}>
              <span className="settings-form-control-copy">
                <strong>{field.label}{field.required ? ' *' : ''}</strong>
                {field.description ? <small id={`workflow-field-${field.id}-description`}>{field.description}</small> : null}
              </span>
              <div className="workflow-field-preview-control">
                {field.multiline ? <textarea aria-label={field.label} aria-describedby={field.description ? `workflow-field-${field.id}-description` : undefined}
                  aria-required={field.required} maxLength={4000} rows={3} placeholder={field.placeholder ?? ''}
                  value={fieldValues[field.id] ?? ''} readOnly={historyMode} onChange={(event) => changeFieldValues({ ...fieldValues, [field.id]: event.target.value })} />
                  : <input type="text" aria-label={field.label} aria-describedby={field.description ? `workflow-field-${field.id}-description` : undefined}
                    aria-required={field.required} maxLength={4000} placeholder={field.placeholder ?? ''}
                    value={fieldValues[field.id] ?? ''} readOnly={historyMode} onChange={(event) => changeFieldValues({ ...fieldValues, [field.id]: event.target.value })}
                    onKeyDown={(event) => { if (event.key === 'Enter') event.preventDefault(); }} />}
                {!historyMode ? <button type="button" className="workflow-heading-edit-button" aria-label={`Configure ${field.label}`}
                  title={`Configure ${field.label}`} disabled={busy} onClick={() => setFieldDialog({ index, field })}><Pencil size={15} aria-hidden="true" /></button>
                  : null}
              </div>
            </div>)}
        </div>
      </div>
    </section>
    <section className="workflow-cell-runbook" aria-label="Notebook cells">
      {builtIn ? <p className="workflow-empty-note">The Repository Auditor keeps its six cell IDs and order so its scope and coverage checks remain active. Cell content and language are editable.</p> : null}
      {viewNotebook.cells.map((cell, index) => <article className={`workflow-cell workflow-cell-${cell.cell_type}`} key={cell.id}>
        <header className="workflow-cell-heading"><div><small>{cell.cell_type === 'code' ? 'Code' : 'Markdown'} · Cell {index + 1}</small><code>{cell.id}</code></div>
          <span className={`workflow-cell-run-status workflow-cell-run-status-${workflowCellRunStatus(displayedAssignment, cell.id, index, sessionStatus).toLowerCase().replace(/\s+/gu, '-')}`}>
            {workflowCellRunStatus(displayedAssignment, cell.id, index, sessionStatus)}
          </span>
          {!builtIn && !historyMode ? <div className="workflow-cell-controls">
            <button type="button" className="secondary-button" disabled={index === 0} onClick={() => moveCell(index, -1)}>Move up</button>
            <button type="button" className="secondary-button" disabled={index === notebook.cells.length - 1} onClick={() => moveCell(index, 1)}>Move down</button>
            <button type="button" className="secondary-button" disabled={notebook.cells.length === 1} onClick={() => setNotebook((current) => ({ ...current, cells: current.cells.filter((_, position) => position !== index) }))}>Remove</button>
          </div> : null}</header>
        <div className="workflow-cell-settings">
          <label>Cell title<input required readOnly={historyMode} maxLength={160} value={cell.metadata.beale.title}
            onChange={(event) => updateCell(index, { metadata: { ...cell.metadata, beale: { ...cell.metadata.beale, title: event.target.value } } })} /></label>
          <label>Cell type<select disabled={historyMode} value={cell.cell_type} onChange={(event) => {
            const kind = event.target.value as 'markdown' | 'code';
            updateCell(index, kind === 'code'
              ? { cell_type: 'code', metadata: { beale: { title: cell.metadata.beale.title, language: 'python3' }, vscode: { languageId: 'python3' } }, execution_count: null, outputs: [] }
              : { cell_type: 'markdown', metadata: { beale: { title: cell.metadata.beale.title } }, execution_count: undefined, outputs: undefined });
          }}><option value="markdown">Markdown</option><option value="code">Code</option></select></label>
          {cell.cell_type === 'code' ? <label>Language<select disabled={historyMode} value={cell.metadata.beale.language ?? 'python3'} onChange={(event) => updateCell(index, {
            metadata: { beale: { ...cell.metadata.beale, language: event.target.value }, vscode: { languageId: event.target.value } }
          })}>{CODE_LANGUAGES.map(([value, label]) => <option value={value} key={value}>{label}</option>)}</select></label> : null}
        </div>
        <label>{cell.cell_type === 'code' ? 'Code' : 'Markdown'}<textarea required readOnly={historyMode} maxLength={8000} rows={8} spellCheck={cell.cell_type === 'markdown'}
          className={cell.cell_type === 'code' ? 'workflow-cell-code-input' : undefined} value={cell.source.join('')}
          onChange={(event) => updateCell(index, { source: notebookSourceLines(event.target.value) })} /></label>
        <div className="workflow-cell-preview" aria-label={`Preview for cell ${index + 1}`}>
          {cell.source.join('').trim() ? cell.cell_type === 'code'
            ? renderHighlightedCodeBlock(cell.source.join(''), cell.metadata.beale.language ?? 'python3')
            : renderTraceProseText(cell.source.join(''), 'agent_output') : <span>Preview appears here.</span>}
        </div>
      </article>)}
      {!builtIn && !historyMode ? <div className="workflow-cell-add-actions">
        <button type="button" className="secondary-button" disabled={notebook.cells.length >= 40} onClick={() => setNotebook((current) => ({ ...current, cells: [...current.cells, newNotebookCell('markdown')] }))}>Add markdown</button>
        <button type="button" className="secondary-button" disabled={notebook.cells.length >= 40} onClick={() => setNotebook((current) => ({ ...current, cells: [...current.cells, newNotebookCell('code')] }))}>Add code</button>
      </div> : null}
    </section>
    {error ? <p role="alert" className="error-box">{error}</p> : null}
    {!historyMode ? <div className="workflow-editor-footer"><button type="submit" className="primary-button" disabled={busy || !dirty}>{busy ? 'Saving…' : creating ? 'Create workflow' : 'Save changes'}</button></div> : null}
  </form>
    {fieldDialog ? <WorkflowFieldDialog key={fieldDialog.field.id} field={fieldDialog.field} creating={fieldDialog.index === null}
      fixedAuditorField={builtIn && (fieldDialog.field.id === 'systems' || fieldDialog.field.id === 'focus')}
      onClose={() => setFieldDialog(null)} onApply={applyField} onRemove={removeField} /> : null}</>}
    </div>
  </main>;
}

export function WorkflowFieldDialog({ field, creating, fixedAuditorField, onClose, onApply, onRemove }: {
  field: SessionWorkflowField;
  creating: boolean;
  fixedAuditorField: boolean;
  onClose: () => void;
  onApply: (field: SessionWorkflowField) => void;
  onRemove: () => void;
}): JSX.Element {
  const [draft, setDraft] = useState(field);
  const formId = `workflow-field-dialog-${field.id}`;
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (!draft.label.trim()) return;
    onApply({ id: field.id, label: draft.label.trim(), required: draft.required,
      ...(draft.description?.trim() ? { description: draft.description.trim() } : {}),
      ...(draft.placeholder?.trim() ? { placeholder: draft.placeholder.trim() } : {}),
      ...(draft.multiline ? { multiline: true } : {}) });
  };
  return <Modal className="workflow-field-dialog" title={creating ? 'Add a field' : `Configure ${field.label}`} onClose={onClose}
    footer={<>
      {!creating && !fixedAuditorField ? <button type="button" className="secondary-button modal-footer-leading" onClick={onRemove}>Remove field</button> : null}
      <button type="button" className="secondary-button" onClick={onClose}>Cancel</button>
      <button type="submit" form={formId} className="primary-button" disabled={!draft.label.trim()}>{creating ? 'Add field' : 'Apply field'}</button>
    </>}>
    <form id={formId} className="workflow-field-dialog-form" onSubmit={submit}>
      <label>Field title<input autoFocus required maxLength={160} value={draft.label} onChange={(event) => setDraft((current) => ({ ...current, label: event.target.value }))} /></label>
      <label>Short description<input maxLength={240} value={draft.description ?? ''} onChange={(event) => setDraft((current) => ({ ...current, description: event.target.value }))} /></label>
      <label>Placeholder<input maxLength={240} value={draft.placeholder ?? ''} onChange={(event) => setDraft((current) => ({ ...current, placeholder: event.target.value }))} /></label>
      <fieldset>
        <legend>Input size</legend>
        <label><input type="radio" name={`${formId}-size`} checked={!draft.multiline} disabled={fixedAuditorField}
          onChange={() => setDraft((current) => ({ ...current, multiline: false }))} /> One line</label>
        <label><input type="radio" name={`${formId}-size`} checked={draft.multiline === true} disabled={fixedAuditorField}
          onChange={() => setDraft((current) => ({ ...current, multiline: true }))} /> Multiple lines</label>
      </fieldset>
      <label className="workflow-field-dialog-required"><input type="checkbox" checked={draft.required} disabled={fixedAuditorField}
        onChange={(event) => setDraft((current) => ({ ...current, required: event.target.checked }))} /> Required at session start</label>
      {fixedAuditorField ? <p>The Repository Auditor requires this field's input size and requirement to stay fixed.</p> : null}
    </form>
  </Modal>;
}

function newFieldId(): string { return `field_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`; }
function newCellId(): string { return `cell_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`; }
