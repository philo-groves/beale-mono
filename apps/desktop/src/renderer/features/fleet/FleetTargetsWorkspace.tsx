import { useState } from 'react';
import type { JSX } from 'react';
import { Box, Globe2, Monitor, RefreshCw, ServerCog } from 'lucide-react';
import type { FleetState } from '@beale/app-server-runtime/protocol';
import type { WorkspaceRegistryEntry } from '@shared/types';
import { availableFleetTargets, configuredWorkspaceTargets } from '../../../shared/fleet';

interface TargetRow {
  id: string;
  name: string;
  detail: string;
  available: boolean;
  ready: boolean;
  /** Shown in place of the generic setup hint when the target is not ready. */
  setupHint?: string;
}

export function nextWorkspaceTargetIds(state: FleetState, workspaceId: string, targetId: string, enabled: boolean): string[] {
  const available = new Set(availableFleetTargets(state).map((target) => target.id));
  const current = configuredWorkspaceTargets(state, workspaceId).filter((id) => available.has(id));
  return enabled ? [...new Set([...current, targetId])] : current.filter((id) => id !== targetId);
}

export function FleetTargetsWorkspace({ state, workspaces, selectedWorkspaceId, busy, onScopeChange, onStateChange, onRefresh }: {
  state: FleetState;
  workspaces: readonly WorkspaceRegistryEntry[];
  selectedWorkspaceId: string | null;
  busy: boolean;
  onScopeChange: (workspaceId: string | null) => void;
  onStateChange: (state: FleetState) => void;
  onRefresh: () => Promise<void>;
}): JSX.Element {
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const workspaceList = [...new Map(workspaces.filter((workspace) => workspace.workspaceId)
    .map((workspace) => [workspace.workspaceId, workspace] as const)).values()];
  const currentWorkspace = workspaceList.find((workspace) => workspace.workspaceId === selectedWorkspaceId);
  const scopeId = currentWorkspace?.workspaceId ?? null;
  const scopeWorkspaceIds = scopeId ? [scopeId] : workspaceList.map((workspace) => workspace.workspaceId);
  const availableIds = new Set(availableFleetTargets(state).map((target) => target.id));
  const selectedCount = (targetId: string): number => scopeWorkspaceIds.filter((workspaceId) =>
    configuredWorkspaceTargets(state, workspaceId).includes(targetId)).length;
  const toggle = async (targetId: string, enabled: boolean): Promise<void> => {
    if (saving || busy || scopeWorkspaceIds.length === 0 || !availableIds.has(targetId)) return;
    setSaving(true);
    setError(null);
    let next = state;
    try {
      for (const workspaceId of scopeWorkspaceIds) {
        const targetIds = nextWorkspaceTargetIds(next, workspaceId, targetId, enabled);
        next = await window.beale.configureFleet({ action: 'set-workspace-targets', workspaceId, targetIds });
      }
      onStateChange(next);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
      try { await onRefresh(); } catch { /* Keep the save error visible. */ }
    } finally {
      setSaving(false);
    }
  };
  const refresh = async (): Promise<void> => {
    setError(null);
    try { await onRefresh(); }
    catch (caught) { setError(caught instanceof Error ? caught.message : String(caught)); }
  };
  const targetRow = (target: TargetRow): JSX.Element => {
    const count = selectedCount(target.id);
    const allSelected = scopeWorkspaceIds.length > 0 && count === scopeWorkspaceIds.length;
    const detail = !target.ready ? `${target.detail} · ${target.setupHint ?? 'Configure in Fleet before use'}`
      : scopeId ? target.detail : `${target.detail} · ${count} of ${scopeWorkspaceIds.length} workspaces`;
    return <label className="settings-form-control-row fleet-target-row" key={target.id}>
      <span className="settings-form-control-copy"><strong>{target.name}</strong><small>{detail}</small></span>
      <input type="checkbox" aria-label={`${target.name} for ${scopeId ? currentWorkspace?.workspaceName : 'all workspaces'}`}
        checked={allSelected} aria-checked={!scopeId && count > 0 && !allSelected ? 'mixed' : allSelected}
        ref={(element) => { if (element) element.indeterminate = !scopeId && count > 0 && !allSelected; }}
        disabled={busy || saving || !target.available || scopeWorkspaceIds.length === 0}
        onChange={(event) => void toggle(target.id, event.currentTarget.checked)} />
    </label>;
  };
  const browserTargets: TargetRow[] = state.browsers.filter((browser) => browser.id !== 'in-agent').map((browser) => ({
    id: `browser:${browser.id}`, name: browser.name, detail: 'Separate browser profile',
    available: state.enabled && browser.enabled, ready: state.enabled && browser.enabled
  }));
  const vmTargets: TargetRow[] = state.role === 'primary' ? state.machines.flatMap((machine) => [
    ...(machine.canRun ? [{ id: `vm:run:${machine.id}`, name: `${machine.name} · Run`, detail: 'Run commands in this VM',
      available: state.enabled, ready: machine.sshConfigured && machine.state !== 'unknown' }] : []),
    ...(machine.canClone ? [{ id: `vm:clone:${machine.id}`, name: `${machine.name} · Clone`, detail: 'Clone this stopped VM',
      available: state.enabled, ready: machine.sshConfigured && machine.state === 'stopped' }] : [])
  ]) : [];
  const sandboxTargets: TargetRow[] = state.role === 'primary' ? state.sandboxes.filter((sandbox) => sandbox.enabled).map((sandbox) => ({
    id: `sandbox:${sandbox.provider}`, name: `${sandbox.name} sandbox`, detail: 'Fresh sandbox for each session',
    available: state.enabled, ready: sandbox.credentialsConfigured,
    setupHint: 'Set the provider API key in the app server environment'
  })) : [];
  const unconfiguredMachines = state.role === 'primary'
    ? state.machines.filter((machine) => !machine.canRun && !machine.canClone) : [];
  return <div className="fleet-targets-workspace wide-content-container" aria-label="Fleet targets">
    <div className="fleet-workspace-tabs research-side-view-tabs research-side-view-tabs-scrollable pill-view-tabs" role="tablist" aria-label="Fleet workspace scope">
      {[{ id: null, key: 'all', name: 'All Fleet' }, ...workspaceList.map((workspace) => ({
        id: workspace.workspaceId, key: workspace.id, name: workspace.workspaceName
      }))].map((scope) => <div className={`research-side-view-tab provider-settings-tab fleet-workspace-tab${scopeId === scope.id ? ' active' : ''}`} key={scope.key}>
        <button type="button" className="research-side-view-tab-activate" role="tab" aria-selected={scopeId === scope.id}
          aria-controls="fleet-targets-panel" onClick={() => onScopeChange(scope.id)}><span>{scope.name}</span></button>
      </div>)}
    </div>
    <header className="resource-workspace-heading">
      <h1>{currentWorkspace ? `${currentWorkspace.workspaceName} Fleet` : 'All Fleet'}</h1>
      <p>{currentWorkspace ? 'Choose which local execution targets are available to this workspace.'
        : 'View the fleet and set execution targets across all workspaces.'}</p>
    </header>
    <div className="fleet-target-groups" id="fleet-targets-panel" role="tabpanel" aria-busy={busy || saving}>
      {state.error ? <p className="settings-form-error" role="status">{state.error}</p> : null}
      {workspaceList.length === 0 ? <p className="fleet-target-empty">Create a workspace to assign execution targets.</p> : null}
      <section className="settings-form fleet-target-group" aria-label="App Servers">
        <header className="settings-form-heading"><div className="fleet-target-heading-label"><ServerCog size={15} aria-hidden="true" /><h2>App Servers</h2></div></header>
        <div className="settings-form-control-list">
          {targetRow({ id: 'local', name: 'Local', detail: 'Run on this machine', available: true, ready: true })}
          {state.appServers.map((server) => <div className="settings-form-control-row fleet-target-row" key={server.id}>
            <span className="settings-form-control-copy"><strong>{server.name}</strong><small>{server.url}</small></span>
            <span className="fleet-target-note">Remote host</span>
          </div>)}
        </div>
      </section>
      <section className="settings-form fleet-target-group" aria-label="Browsers">
        <header className="settings-form-heading"><div className="fleet-target-heading-label"><Globe2 size={15} aria-hidden="true" /><h2>Browsers</h2></div></header>
        <div className="settings-form-control-list">
          {browserTargets.length ? browserTargets.map(targetRow) : <p className="fleet-target-empty">No separate browser profiles found.</p>}
        </div>
      </section>
      <section className="settings-form fleet-target-group" aria-label="Virtual Machines">
        <header className="settings-form-heading"><div className="fleet-target-group-title-row"><div className="fleet-target-heading-label"><Monitor size={15} aria-hidden="true" /><h2>Virtual Machines</h2></div>
          <button type="button" className="secondary-button" disabled={busy || saving} onClick={() => void refresh()}>
            <RefreshCw size={14} aria-hidden="true" />Refresh
          </button></div></header>
        <div className="settings-form-control-list">
          {vmTargets.map(targetRow)}
          {unconfiguredMachines.map((machine) => <div className="settings-form-control-row fleet-target-row" key={machine.id}>
            <span className="settings-form-control-copy"><strong>{machine.name}</strong><small>Configure run or clone in the sidebar to use this VM.</small></span>
          </div>)}
          {state.remoteMachines.map((machine) => <div className="settings-form-control-row fleet-target-row" key={`remote:${machine.id}`}>
            <span className="settings-form-control-copy"><strong>{machine.name}</strong><small>Remote VM · managed by its app server</small></span>
          </div>)}
          {!vmTargets.length && !unconfiguredMachines.length && !state.remoteMachines.length
            ? <p className="fleet-target-empty">No virtual machines found. Refresh Fleet to discover VMs.</p> : null}
        </div>
      </section>
      <section className="settings-form fleet-target-group" aria-label="Sandboxes">
        <header className="settings-form-heading"><div className="fleet-target-heading-label"><Box size={15} aria-hidden="true" /><h2>Sandboxes</h2></div></header>
        <div className="settings-form-control-list">
          {sandboxTargets.length ? sandboxTargets.map(targetRow)
            : <p className="fleet-target-empty">Enable a sandbox provider in the Fleet sidebar to add it here.</p>}
        </div>
      </section>
      {error ? <p className="settings-form-error" role="alert">{error}</p> : null}
    </div>
  </div>;
}
