import { useCallback, useEffect, useState } from 'react';
import type { FormEvent, JSX } from 'react';
import { Plus, RefreshCw, RotateCw, Settings2, X } from 'lucide-react';
import type { FleetAppServer, FleetMachine, FleetSshTestResult, FleetState } from '@beale/app-server-runtime/protocol';

export function FleetSettingsView(): JSX.Element {
  const [state, setState] = useState<FleetState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [openMachineId, setOpenMachineId] = useState<string | null>(null);
  const [openServerId, setOpenServerId] = useState<string | null>(null);
  const [addingServer, setAddingServer] = useState(false);
  const [restartingId, setRestartingId] = useState<string | null>(null);

  const refresh = useCallback(async (): Promise<void> => {
    try {
      setState(await window.beale.getFleetState());
      setError(null);
    } catch (caught) {
      setError(message(caught));
    }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);

  const change = async (action: () => Promise<FleetState>): Promise<boolean> => {
    if (busy) return false;
    setBusy(true);
    setError(null);
    try {
      setState(await action());
      return true;
    } catch (caught) {
      setError(message(caught));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const restart = async (id: string, action: () => Promise<void>): Promise<void> => {
    if (busy) return;
    setBusy(true);
    setRestartingId(id);
    setError(null);
    try {
      await action();
      await refresh();
    } catch (caught) {
      setError(message(caught));
    } finally {
      setBusy(false);
      setRestartingId(null);
    }
  };

  const localServerRow = <div className="settings-form-control-row fleet-machine-row">
    <span className="settings-form-control-copy"><strong>Local App Server</strong><small>This machine’s Beale app-server process.</small></span>
    <button type="button" className="fleet-machine-configure-button" aria-label="Restart local app server" title="Restart local app server" disabled={busy} onClick={() => void restart('local', () => window.beale.restartLocalAppServer())}>
      <RotateCw size={18} aria-hidden="true" />
    </button>
  </div>;

  if (!state) {
    return <div className="settings-page fleet-settings-page"><section className="settings-form"><header className="settings-form-heading"><h2>App Servers</h2></header><div className="settings-form-squircle"><div className="settings-form-control-list">{localServerRow}</div></div><p className={error ? 'settings-form-error' : 'fleet-machine-empty'} role={error ? 'alert' : 'status'}>{error ?? 'Loading Fleet…'}</p></section></div>;
  }
  const openMachine = state.role === 'primary' ? state.machines.find((machine) => machine.id === openMachineId) : undefined;
  const openServer = state.appServers.find((server) => server.id === openServerId);
  return (
    <div className="settings-page fleet-settings-page">
      <section className="settings-form" aria-busy={busy}>
        <header className="settings-form-heading">
          <h2 id="fleet-configuration-heading">Fleet</h2>
          <p>Choose how this Beale instance participates in Fleet research.</p>
        </header>
        <fieldset className="settings-form-squircle" aria-labelledby="fleet-configuration-heading" disabled={busy}>
          <div className="settings-form-control-list">
            <label className="settings-form-control-row">
              <span className="settings-form-control-copy"><strong>Enable Fleet</strong><small>Make Fleet machines and tools available to research.</small></span>
              <input type="checkbox" checked={state.enabled} onChange={(event) => {
                const enabled = event.currentTarget.checked;
                void change(() => window.beale.configureFleet({ action: 'set-enabled', enabled }));
              }} />
            </label>
            <label className="settings-form-control-row">
              <span className="settings-form-control-copy"><strong>This Beale instance</strong><small>Primary manages VMs; a guest reports its primary connection.</small></span>
              <select aria-label="Fleet role" value={state.role} onChange={(event) => {
                const role = event.currentTarget.value;
                void change(() => window.beale.configureFleet({ action: 'set-role', role }));
              }}>
                <option value="primary">Primary</option><option value="guest">VM guest</option>
              </select>
            </label>
            {state.role === 'guest' ? (
              <div className="settings-form-control-row">
                <span className="settings-form-control-copy"><strong>Primary machine</strong><small>{state.primary?.name ?? 'No primary machine has connected.'}{state.primary?.sshHost ? ` · SSH host ${state.primary.sshHost}` : ''}</small></span>
              </div>
            ) : null}
          </div>
        </fieldset>
      </section>

      <section className="settings-form" aria-busy={busy}>
        <header className="settings-form-heading">
          <h2 id="fleet-app-servers-heading">App Servers</h2>
          <p>Connect to other Beale app servers on this Tailscale network. Configure each server and its VMs on its own machine.</p>
        </header>
        <div className="settings-form-squircle" aria-labelledby="fleet-app-servers-heading">
          <div className="settings-form-control-list fleet-machine-list" aria-label="Remote app servers">
            {localServerRow}
            {state.appServers.length === 0 ? <p className="fleet-machine-empty">No remote app servers are saved.</p> : null}
            {state.appServers.map((server) => (
              <div className="settings-form-control-row fleet-machine-row" key={server.id}>
                <span className="settings-form-control-copy"><strong>{server.name}</strong><small>{server.url}</small></span>
                <span className="fleet-machine-actions">
                  <button type="button" className="fleet-machine-configure-button" aria-label={`Restart ${server.name} app server`} title={`Restart ${server.name} app server`} disabled={busy || state.role !== 'primary'} onClick={() => void restart(server.id, () => window.beale.restartFleetAppServer(server.id))}>
                    <RotateCw size={18} aria-hidden="true" />
                  </button>
                  <button type="button" className="fleet-machine-configure-button" aria-label={`Configure ${server.name}`} title={`Configure ${server.name}`} disabled={busy || state.role !== 'primary'} onClick={() => { setError(null); setOpenServerId(server.id); }}>
                    <Settings2 size={18} aria-hidden="true" />
                  </button>
                </span>
              </div>
            ))}
          </div>
        </div>
        {state.role === 'primary' ? <div className="settings-form-actions fleet-machine-refresh"><span /><button type="button" disabled={busy} onClick={() => setAddingServer(true)}><Plus size={14} aria-hidden="true" />Add App Server</button></div> : null}
      </section>

      <section className="settings-form" aria-busy={busy}>
        <header className="settings-form-heading">
          <h2 id="fleet-machines-heading">Virtual Machines</h2>
          <p>{state.role === 'primary' ? 'Configure operator-prepared VMs and clone registered bases.' : 'The primary initiates session control and file transfers to this guest.'}</p>
        </header>
        <div className="settings-form-squircle" aria-labelledby="fleet-machines-heading">
          {state.role === 'guest' ? (
            <p className="fleet-machine-empty">Guest instances do not list child VMs.</p>
          ) : (
            <div className="settings-form-control-list fleet-machine-list" aria-label="Fleet machines">
              {state.error ? <p className="fleet-machine-empty" role="status">{state.error}</p> : null}
              {state.machines.length === 0 ? <p className="fleet-machine-empty">No VMs were found. Prepare one in Tart or Hyper-V, then refresh Fleet.</p> : null}
              {state.machines.map((machine) => (
                <div className="settings-form-control-row fleet-machine-row" key={machine.id}>
                  <span className="settings-form-control-copy">
                    <strong>{machine.name}</strong>
                    <small>{machine.backend} · {machine.state} · {machine.base ? 'Base VM' : 'Worker'} · {machine.privilege} privilege · {machine.owner ? 'Reserved by a session' : machine.sshConfigured ? 'SSH user saved in Beale' : 'Set SSH user in Beale'}</small>
                  </span>
                  <span className="fleet-machine-actions">
                    {!machine.base && machine.state === 'running' && machine.sshConfigured ? <button type="button" className="fleet-machine-configure-button" aria-label={`Restart ${machine.name} app server`} title={`Restart ${machine.name} app server`} disabled={busy} onClick={() => void restart(machine.id, () => window.beale.restartFleetGuestAppServer(machine.id))}>
                      <RotateCw size={18} aria-hidden="true" />
                    </button> : null}
                    <button type="button" className="fleet-machine-configure-button" aria-label={`Configure ${machine.name}`} title={`Configure ${machine.name}`} disabled={busy} onClick={() => { setError(null); setOpenMachineId(machine.id); }}>
                      <Settings2 size={18} aria-hidden="true" />
                    </button>
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>
        {error && !openMachine ? <p className="settings-form-error" role="alert">{error}</p> : null}
        {restartingId ? <p role="status" className="fleet-machine-empty">Restarting app-server…</p> : null}
        <div className="settings-form-actions fleet-machine-refresh">
          <span />
          <button type="button" disabled={busy} onClick={() => void refresh()}><RefreshCw size={14} aria-hidden="true" />Refresh</button>
        </div>
      </section>

      {openMachine ? (
        <FleetMachineDialog
          key={openMachine.id}
          machine={openMachine}
          busy={busy}
          error={error}
          onClose={() => { setOpenMachineId(null); setError(null); }}
          onChange={change}
        />
      ) : null}
      {addingServer || openServer ? (
        <FleetAppServerDialog
          key={openServer?.id ?? 'new'} server={openServer} busy={busy} error={error}
          onClose={() => { setAddingServer(false); setOpenServerId(null); setError(null); }} onChange={change}
        />
      ) : null}
    </div>
  );
}

function FleetAppServerDialog({ server, busy, error, onClose, onChange }: {
  server: FleetAppServer | undefined;
  busy: boolean;
  error: string | null;
  onClose: () => void;
  onChange: (action: () => Promise<FleetState>) => Promise<boolean>;
}): JSX.Element {
  const [name, setName] = useState(server?.name ?? '');
  const [url, setUrl] = useState(server?.url ?? '');
  const [operatorToken, setOperatorToken] = useState('');
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ success: boolean; message: string } | null>(null);
  const locked = busy || testing;
  const test = async (): Promise<void> => {
    setTesting(true);
    setTestResult(null);
    try { setTestResult(await window.beale.testFleetAppServer({ ...(server ? { serverId: server.id } : {}), url: url.trim(), operatorToken: operatorToken.trim() })); }
    catch (caught) { setTestResult({ success: false, message: message(caught) }); }
    finally { setTesting(false); }
  };
  const save = async (event: FormEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    const saved = await onChange(() => window.beale.configureFleet({
      action: 'set-app-server', serverId: server?.id ?? crypto.randomUUID(), name: name.trim(), url: url.trim(), operatorToken: operatorToken.trim(),
    }));
    if (saved) onClose();
  };
  const remove = async (): Promise<void> => {
    if (!server) return;
    const removed = await onChange(() => window.beale.configureFleet({ action: 'remove-app-server', serverId: server.id }));
    if (removed) onClose();
  };
  return (
    <div className="modal-backdrop fleet-vm-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !locked) onClose(); }}>
      <form className="modal-panel fleet-vm-dialog" role="dialog" aria-modal="true" aria-label={server ? `Configure ${server.name}` : 'Add App Server'} onSubmit={(event) => void save(event)}>
        <header className="modal-header"><h2>{server ? `Configure ${server.name}` : 'Add App Server'}</h2><button type="button" className="icon-button" aria-label="Close" disabled={locked} onClick={onClose}><X size={18} /></button></header>
        <div className="modal-body">
          <div className="fleet-vm-dialog-fields">
            <label className="fleet-vm-dialog-field"><span>Name</span><input value={name} disabled={locked} onChange={(event) => setName(event.currentTarget.value)} placeholder="Research machine" required /></label>
            <label className="fleet-vm-dialog-field"><span>Tailscale HTTPS URL</span><input value={url} disabled={locked} onChange={(event) => { setUrl(event.currentTarget.value); setTestResult(null); }} placeholder="https://machine.tailnet.ts.net:47174" required /></label>
            <label className="fleet-vm-dialog-field"><span>Operator token</span><input type="password" value={operatorToken} disabled={locked} onChange={(event) => { setOperatorToken(event.currentTarget.value); setTestResult(null); }} placeholder={server ? 'Saved; enter a new token to replace' : 'Token from the remote app server'} required={!server} /><small>Stored only on this machine. The remote server must already have Tailscale Serve enabled.</small></label>
          </div>
          <div className="fleet-vm-dialog-actions"><button type="button" className="secondary-button" disabled={locked || !url.trim() || (!server && !operatorToken.trim())} onClick={() => void test()}>{testing ? 'Testing…' : 'Test Connection'}</button>
            {testResult ? <p className={testResult.success ? 'fleet-vm-test-success' : 'settings-form-error'} role="status">{testResult.message}</p> : null}
          </div>
          {error ? <p className="settings-form-error" role="alert">{error}</p> : null}
        </div>
        <footer className="modal-footer">
          {server ? <button type="button" className="secondary-button" disabled={locked} onClick={() => void remove()}>Remove</button> : null}
          <button type="button" className="secondary-button" disabled={locked} onClick={onClose}>Cancel</button>
          <button type="submit" className="primary-button" disabled={locked}>Save App Server</button>
        </footer>
      </form>
    </div>
  );
}

function FleetMachineDialog({ machine, busy, error, onClose, onChange }: {
  machine: FleetMachine;
  busy: boolean;
  error: string | null;
  onClose: () => void;
  onChange: (action: () => Promise<FleetState>) => Promise<boolean>;
}): JSX.Element {
  const [base, setBase] = useState(machine.base);
  const [privilege, setPrivilege] = useState(machine.privilege);
  const [sshHost, setSshHost] = useState(machine.sshHost ?? '');
  const [sshUser, setSshUser] = useState(machine.sshUser ?? '');
  const [sshIdentityFile, setSshIdentityFile] = useState('');
  const [clearIdentity, setClearIdentity] = useState(false);
  const [sshKnownHostsFile, setSshKnownHostsFile] = useState('');
  const [clearKnownHosts, setClearKnownHosts] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<FleetSshTestResult | null>(null);
  const [cloneName, setCloneName] = useState('');
  const locked = busy || testing;

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent): void => {
      if (event.key === 'Escape' && !locked) onClose();
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [locked, onClose]);

  const saveConfiguration = (): Promise<boolean> => onChange(() => window.beale.configureFleet({
      action: 'set-machine', machineId: machine.id, base, privilege,
      sshHost: sshHost.trim(), sshUser: sshUser.trim(),
      ...(clearIdentity || sshIdentityFile.trim() ? { sshIdentityFile: clearIdentity ? '' : sshIdentityFile.trim() } : {}),
      ...(clearKnownHosts || sshKnownHostsFile.trim() ? { sshKnownHostsFile: clearKnownHosts ? '' : sshKnownHostsFile.trim() } : {}),
    }));

  const testConnection = async (): Promise<void> => {
    if (locked) return;
    setTesting(true);
    setTestResult(null);
    try {
      setTestResult(await window.beale.testFleetVmConnection({
        machineId: machine.id,
        sshHost: sshHost.trim(),
        sshUser: sshUser.trim(),
        ...(clearIdentity || sshIdentityFile.trim() ? { sshIdentityFile: clearIdentity ? '' : sshIdentityFile.trim() } : {}),
        ...(clearKnownHosts || sshKnownHostsFile.trim() ? { sshKnownHostsFile: clearKnownHosts ? '' : sshKnownHostsFile.trim() } : {}),
      }));
    } catch (caught) {
      setTestResult({ success: false, message: message(caught) });
    } finally {
      setTesting(false);
    }
  };

  const save = async (event: FormEvent<HTMLFormElement>): Promise<void> => {
    event.preventDefault();
    const saved = await saveConfiguration();
    if (saved) onClose();
  };

  const clone = async (): Promise<void> => {
    if (!await saveConfiguration()) return;
    const cloned = await onChange(() => window.beale.cloneFleetVm(machine.id, cloneName.trim()));
    if (cloned) setCloneName('');
  };

  return (
    <div className="modal-backdrop fleet-vm-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget && !locked) onClose();
    }}>
      <form className="modal-panel fleet-vm-dialog" role="dialog" aria-modal="true" aria-label={`Configure ${machine.name}`} onSubmit={(event) => void save(event)}>
        <header className="modal-header">
          <h2>Configure {machine.name}</h2>
          <button type="button" autoFocus aria-label="Close VM configuration" disabled={locked} onClick={onClose}><X size={18} aria-hidden="true" /></button>
        </header>
        <div className="modal-body">
          <p className="fleet-vm-dialog-summary">{machine.backend} · {machine.state}{machine.owner ? ' · Reserved by a session' : ''}</p>
          <div className="fleet-vm-dialog-fields">
            <label className="settings-form-control-row">
              <span className="settings-form-control-copy"><strong>Base VM</strong><small>Clone only; base VMs cannot run directly.</small></span>
              <input type="checkbox" checked={base} disabled={locked || Boolean(machine.owner) || machine.state !== 'stopped' && !base} onChange={(event) => setBase(event.currentTarget.checked)} />
            </label>
            <label className="settings-form-control-row">
              <span className="settings-form-control-copy"><strong>Privilege</strong><small>Label this VM's manually configured privilege level.</small></span>
              <select value={privilege} disabled={locked || Boolean(machine.owner)} onChange={(event) => setPrivilege(event.currentTarget.value as FleetMachine['privilege'])}><option value="standard">Standard</option><option value="elevated">Elevated</option></select>
            </label>
            <label className="fleet-vm-dialog-field"><span>SSH host override</span><input value={sshHost} disabled={locked || Boolean(machine.owner)} placeholder="Auto-detect VM IP" onChange={(event) => { setSshHost(event.currentTarget.value); setTestResult(null); }} /></label>
            <label className="fleet-vm-dialog-field"><span>SSH user</span><input value={sshUser} disabled={locked || Boolean(machine.owner)} placeholder="Guest account name" onChange={(event) => { setSshUser(event.currentTarget.value); setTestResult(null); }} /><small>Beale needs this account name to connect; this does not change SSH inside the VM.</small></label>
            <label className="fleet-vm-dialog-field"><span>SSH identity file</span><input value={sshIdentityFile} disabled={locked || Boolean(machine.owner) || clearIdentity} placeholder={machine.sshIdentityConfigured ? 'Configured; enter a new path to replace' : 'Optional host path'} onChange={(event) => { setSshIdentityFile(event.currentTarget.value); setTestResult(null); }} /></label>
            {machine.sshIdentityConfigured ? (
              <label className="fleet-vm-clear-identity"><input type="checkbox" checked={clearIdentity} disabled={locked || Boolean(machine.owner)} onChange={(event) => { setClearIdentity(event.currentTarget.checked); setTestResult(null); }} />Clear saved SSH identity file</label>
            ) : null}
            <label className="fleet-vm-dialog-field"><span>SSH known-hosts file</span><input value={sshKnownHostsFile} disabled={locked || Boolean(machine.owner) || clearKnownHosts} placeholder={machine.sshKnownHostsConfigured ? 'Configured; enter a new path to replace' : 'Optional host path'} onChange={(event) => { setSshKnownHostsFile(event.currentTarget.value); setTestResult(null); }} /><small>For Tart, Beale uses ~/.ssh/tart_known_hosts when it exists. A first connection to a clone records its host key there.</small></label>
            {machine.sshKnownHostsConfigured ? (
              <label className="fleet-vm-clear-identity"><input type="checkbox" checked={clearKnownHosts} disabled={locked || Boolean(machine.owner)} onChange={(event) => { setClearKnownHosts(event.currentTarget.checked); setTestResult(null); }} />Clear saved known-hosts file</label>
            ) : null}
          </div>
          <div className="fleet-vm-dialog-actions">
            <button type="button" className="secondary-button" disabled={locked} onClick={() => void testConnection()}>{testing ? 'Testing…' : 'Test SSH'}</button>
            {testResult ? <p className={testResult.success ? 'fleet-vm-test-success' : 'settings-form-error'} role="status">{testResult.message}</p> : null}
            {machine.base ? (
              <><label className="fleet-vm-dialog-field"><span>Clone name</span><input value={cloneName} disabled={locked} placeholder="New worker VM name" onChange={(event) => setCloneName(event.currentTarget.value)} /></label>
                <button type="button" className="secondary-button" disabled={locked || machine.state !== 'stopped' || !base || !cloneName.trim()} onClick={() => void clone()}>Clone VM</button></>
            ) : (
              <button type="button" className="secondary-button" disabled={locked || Boolean(machine.owner) || machine.state === 'unknown'} onClick={() => void onChange(() => machine.state === 'running' ? window.beale.stopFleetVm(machine.id) : window.beale.startFleetVm(machine.id))}>{machine.state === 'running' ? 'Stop VM' : 'Start VM'}</button>
            )}
          </div>
          {error ? <p className="settings-form-error" role="alert">{error}</p> : null}
        </div>
        <footer className="modal-footer">
          <button type="button" className="secondary-button" disabled={locked} onClick={onClose}>Cancel</button>
          <button type="submit" className="primary-button" disabled={locked || Boolean(machine.owner)}>Save VM</button>
        </footer>
      </form>
    </div>
  );
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
