import { useEffect, useState } from 'react';
import type { JSX } from 'react';
import type { FleetState } from '@beale/app-server-runtime/protocol';
import { fleetVmRequired } from '../../../shared/fleet';
import { errorMessage } from '../../lib/errors';

export function WorkspaceVmRequirement({ workspaceId, disabled }: {
  workspaceId: string;
  disabled: boolean;
}): JSX.Element | null {
  const [loaded, setLoaded] = useState<{ workspaceId: string; state: FleetState } | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!workspaceId) return;
    let active = true;
    setLoaded(null);
    setError(null);
    void window.beale.getFleetState()
      .then((state) => { if (active) setLoaded({ workspaceId, state }); })
      .catch((caught: unknown) => { if (active) setError(errorMessage(caught)); });
    return () => { active = false; };
  }, [workspaceId]);

  const state = loaded?.workspaceId === workspaceId ? loaded.state : null;
  const changeRequired = async (required: boolean): Promise<void> => {
    if (!state || saving || disabled) return;
    setSaving(true);
    setError(null);
    try {
      const next = await window.beale.configureFleet({ action: 'set-required', workspaceId, required });
      setLoaded({ workspaceId, state: next });
    } catch (caught) {
      setError(errorMessage(caught));
    } finally {
      setSaving(false);
    }
  };

  if (!state || state.role !== 'primary') return error
    ? <p className="workspace-overview-error" role="alert">VM requirement unavailable: {error}</p>
    : null;

  return <>
    <label className="settings-form-control-row workspace-overview-control-row">
      <span className="settings-form-control-copy">
        <strong>Require a VM for this workspace</strong>
        <small>Defaults on when a base VM is registered.</small>
      </span>
      <input
        aria-label="Require a VM for this workspace"
        type="checkbox"
        checked={fleetVmRequired(state, workspaceId)}
        disabled={disabled || saving}
        onChange={(event) => { void changeRequired(event.currentTarget.checked); }}
      />
    </label>
    {error ? <p className="workspace-overview-error" role="alert">{error}</p> : null}
  </>;
}
