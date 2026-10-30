import { useEffect, useRef, useState } from 'react';
import type { FormEvent, JSX } from 'react';
import { QrCode, RefreshCw, RotateCw } from 'lucide-react';
import type { FleetAppServer, FleetMachine, FleetSshTestResult, FleetState } from '@beale/app-server-runtime/protocol';
import type { AppServerRemoteAccessSettings, AppServerRemoteAccessUpdate, WorkspaceRegistryEntry } from '@shared/types';
import { Modal } from '../../app/Modal';
import type { FleetSelection } from '../fleet/FleetSidebar';
import { FleetTargetsWorkspace } from '../fleet/FleetTargetsWorkspace';

export function FleetSettingsView({ state, loadError, remoteAccessSettings, remoteAccessBusy, selection, onSelect, onStateChange, onRefresh, onDetectRemoteAccess, onSaveRemoteAccess, workspaces = [], selectedWorkspaceId = null, onScopeChange = () => undefined }: {
  state: FleetState | null;
  loadError: string | null;
  remoteAccessSettings: AppServerRemoteAccessSettings | null;
  remoteAccessBusy: boolean;
  selection: FleetSelection;
  onSelect: (selection: FleetSelection) => void;
  onStateChange: (state: FleetState) => void;
  onRefresh: () => Promise<void>;
  onDetectRemoteAccess: () => Promise<void>;
  onSaveRemoteAccess: (update: AppServerRemoteAccessUpdate) => Promise<void>;
  workspaces?: readonly WorkspaceRegistryEntry[];
  selectedWorkspaceId?: string | null;
  onScopeChange?: (workspaceId: string | null) => void;
}): JSX.Element {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [restartingId, setRestartingId] = useState<string | null>(null);
  const change = async (action: () => Promise<FleetState>): Promise<boolean> => {
    if (busy) return false;
    setBusy(true);
    setError(null);
    try {
      onStateChange(await action());
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
    try { await action(); await onRefresh(); }
    catch (caught) { setError(message(caught)); }
    finally { setBusy(false); setRestartingId(null); }
  };

  if (!state) return <div className="settings-page fleet-settings-page"><section className="settings-form"><header className="settings-form-heading"><h2>Fleet</h2></header><p className={loadError ? 'settings-form-error' : 'fleet-machine-empty'} role={loadError ? 'alert' : 'status'}>{loadError ?? 'Loading Fleet…'}</p></section></div>;

  const selectedMachine = selection.section === 'machines' ? state.machines.find((machine) => machine.id === selection.itemId) : undefined;
  const selectedServer = selection.section === 'app-servers' ? state.appServers.find((server) => server.id === selection.itemId) : undefined;
  const addingServer = selection.section === 'app-servers' && selection.itemId === 'new';
  const localSelected = selection.section === 'app-servers' && selection.itemId === 'local';
  const browsersSelected = selection.section === 'browsers' && selection.itemId === 'settings';
  const closeDialog = (): void => onSelect({ section: selection.section, itemId: null });
  const dialogTitle = selectedMachine ? `Configure ${selectedMachine.name}`
    : selectedServer ? `Configure ${selectedServer.name}`
      : addingServer ? 'Add App Server'
        : localSelected ? 'Local App Server'
          : browsersSelected ? 'Browsers' : null;
  return <div className="settings-page fleet-settings-page">
    <FleetTargetsWorkspace state={state} workspaces={workspaces} selectedWorkspaceId={selectedWorkspaceId}
      busy={busy} onScopeChange={onScopeChange} onStateChange={onStateChange} onRefresh={onRefresh} />
    {dialogTitle ? <Modal title={dialogTitle} className="fleet-settings-dialog" wide closeDisabled={busy} onClose={closeDialog}>
      {selectedMachine ? <FleetMachineDetails key={selectedMachine.id} machine={selectedMachine} busy={busy} error={error}
        onChange={change} /> : selectedServer || addingServer ? <FleetAppServerDetails
          key={selectedServer?.id ?? 'new'} server={selectedServer} busy={busy} error={error}
          restarting={restartingId === selectedServer?.id}
          onRestart={() => selectedServer ? restart(selectedServer.id, () => window.beale.restartFleetAppServer(selectedServer.id)) : Promise.resolve()}
          onBack={closeDialog} onSaved={closeDialog} onChange={change} /> : localSelected ? <LocalAppServerDetails
            busy={busy} error={error} restarting={restartingId === 'local'} remoteAccessSettings={remoteAccessSettings} remoteAccessBusy={remoteAccessBusy}
            onRestart={() => restart('local', () => window.beale.restartLocalAppServer())}
            onDetectRemoteAccess={onDetectRemoteAccess} onSaveRemoteAccess={onSaveRemoteAccess} /> : <BrowserSettingsView
              key={state.browserHomePage} homePage={state.browserHomePage} browserCount={state.browsers.length}
              busy={busy} error={error} onChange={change} />}
    </Modal> : null}
  </div>;
}

function LocalAppServerDetails({ busy, error, restarting, remoteAccessSettings, remoteAccessBusy, onRestart, onDetectRemoteAccess, onSaveRemoteAccess }: {
  busy: boolean;
  error: string | null;
  restarting: boolean;
  remoteAccessSettings: AppServerRemoteAccessSettings | null;
  remoteAccessBusy: boolean;
  onRestart: () => Promise<void>;
  onDetectRemoteAccess: () => Promise<void>;
  onSaveRemoteAccess: (update: AppServerRemoteAccessUpdate) => Promise<void>;
}): JSX.Element {
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ success: boolean; message: string } | null>(null);
  const [enabled, setEnabled] = useState(remoteAccessSettings?.enabled ?? false);
  const [magicDnsName, setMagicDnsName] = useState(remoteAccessSettings?.magicDnsName ?? '');
  const locked = busy || testing || remoteAccessBusy;
  useEffect(() => {
    setEnabled(remoteAccessSettings?.enabled ?? false);
    setMagicDnsName(remoteAccessSettings?.magicDnsName ?? '');
  }, [remoteAccessSettings]);
  const test = async (): Promise<void> => {
    if (locked) return;
    setTesting(true);
    setTestResult(null);
    try { setTestResult(await window.beale.testLocalAppServerConnection()); }
    catch (caught) { setTestResult({ success: false, message: message(caught) }); }
    finally { setTesting(false); }
  };
  return <section className="settings-form fleet-detail-view" aria-label="Local" aria-busy={locked}>
    <header className="settings-form-heading"><p>This machine’s Beale app-server process.</p></header>
    <form className="settings-form-squircle fleet-detail-form fleet-text-form" onSubmit={(event) => {
      event.preventDefault();
      void onSaveRemoteAccess({ enabled, magicDnsName });
    }}>
      <div className="settings-form-control-list">
        <label className="settings-form-control-row">
          <span className="settings-form-control-copy"><strong>Tailscale Serve</strong><small>Keep the app-server on 127.0.0.1 and let Tailscale terminate HTTPS for iPhone clients.</small></span>
          <input type="checkbox" aria-label="Tailscale Serve" checked={enabled} disabled={locked} onChange={(event) => setEnabled(event.currentTarget.checked)} />
        </label>
        <label className="settings-form-control-row fleet-text-field">
          <span className="settings-form-control-copy"><strong>MagicDNS Name</strong><small>The full device name reported by Tailscale, ending in .ts.net.</small></span>
          <input type="text" aria-label="MagicDNS Name" autoCapitalize="none" autoCorrect="off" spellCheck={false} placeholder="mac-name.tailnet.ts.net" value={magicDnsName} disabled={locked} required={enabled} onChange={(event) => setMagicDnsName(event.currentTarget.value)} />
        </label>
        <label className="settings-form-control-row fleet-text-field">
          <span className="settings-form-control-copy"><strong>Tailscale HTTPS URL</strong><small>Derived from the configured MagicDNS name.</small></span>
          <input type="url" value={remoteAccessSettings?.publicUrl ?? ''} placeholder="Detect or apply a MagicDNS name" readOnly />
        </label>
      </div>
      <div className="fleet-app-server-test-actions">
        <div className="fleet-app-server-test-buttons">
          <button type="button" className="secondary-button" disabled={locked} onClick={() => void test()}>{testing ? 'Testing…' : 'Test Connection'}</button>
          <FleetPairingQrCodeAction serverId="local" serverName="Local" disabled={locked}
            needsRemoteAccess={!remoteAccessSettings?.publicUrl} />
          <button type="button" className="secondary-button" disabled={locked} onClick={() => { setTestResult(null); void onRestart(); }}><RotateCw size={14} aria-hidden="true" />Restart App Server</button>
          <button type="button" className="secondary-button" disabled={locked} onClick={() => void onDetectRemoteAccess()}><RefreshCw size={14} aria-hidden="true" />Detect</button>
        </div>
        {testResult ? <p className={testResult.success ? 'fleet-vm-test-success' : 'settings-form-error'} role="status">{testResult.message}</p> : null}
      </div>
      {remoteAccessSettings?.detail ? <div className="error-box">{remoteAccessSettings.detail}</div> : null}
      {error ? <p className="settings-form-error" role="alert">{error}</p> : null}
      {restarting ? <p role="status" className="fleet-machine-empty">Restarting app-server…</p> : null}
      <div className="settings-form-actions fleet-detail-actions">
        <span className="settings-form-control-copy"><small>Applying this setting restarts the app-server. The dedicated HTTPS port avoids replacing other Tailscale Serve routes.</small></span>
        <button type="submit" className="primary-button" disabled={locked || (enabled && !magicDnsName.trim())}>{remoteAccessBusy ? 'Applying…' : 'Apply'}</button>
      </div>
    </form>
  </section>;
}

function FleetPairingQrCodeAction({ serverId, serverName, disabled, needsRemoteAccess = false }: {
  serverId: string;
  serverName: string;
  disabled: boolean;
  needsRemoteAccess?: boolean;
}): JSX.Element {
  const [open, setOpen] = useState(false);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  useEffect(() => () => { requestId.current += 1; }, []);
  const close = (): void => {
    requestId.current += 1;
    setOpen(false);
    setDataUrl(null);
    setError(null);
  };
  const show = async (): Promise<void> => {
    const currentRequestId = ++requestId.current;
    setDataUrl(null);
    setError(null);
    setOpen(true);
    try {
      const result = await window.beale.getFleetPairingQrCode(serverId);
      if (requestId.current === currentRequestId) setDataUrl(result.dataUrl);
    } catch (caught) {
      if (requestId.current === currentRequestId) setError(message(caught));
    }
  };
  return <>
    <button type="button" className="secondary-button" disabled={disabled} onClick={() => void show()}>
      <QrCode size={14} aria-hidden="true" />Show QR Code
    </button>
    {open ? <FleetPairingQrCodeDialog serverName={serverName} dataUrl={dataUrl} error={error}
      needsRemoteAccess={needsRemoteAccess} onClose={close} /> : null}
  </>;
}

export function FleetPairingQrCodeDialog({ serverName, dataUrl, error, needsRemoteAccess, onClose }: {
  serverName: string;
  dataUrl: string | null;
  error: string | null;
  needsRemoteAccess: boolean;
  onClose: () => void;
}): JSX.Element {
  return <Modal title={`Connect ${serverName} to Beale iOS`} className="fleet-pairing-qr-dialog" onClose={onClose}>
    {error ? <p className="settings-form-error" role="alert">{error}</p> : dataUrl ? <>
      <p>Open Beale on your iPhone, choose Scan QR Code, and point the camera at this screen.</p>
      <img className="fleet-pairing-qr-image" src={dataUrl} alt={`Pairing QR code for ${serverName}`} width="300" height="300" />
      {needsRemoteAccess ? <p className="fleet-pairing-qr-warning">Enable Tailscale Serve below before connecting from iOS.</p> : null}
      <p className="fleet-pairing-qr-security">This code contains the app-server operator token. Scan it only with a trusted device.</p>
    </> : <p role="status">Preparing QR code…</p>}
  </Modal>;
}

function BrowserSettingsView({ homePage, browserCount, busy, error, onChange }: {
  homePage: string;
  browserCount: number;
  busy: boolean;
  error: string | null;
  onChange: (action: () => Promise<FleetState>) => Promise<boolean>;
}): JSX.Element {
  const [url, setUrl] = useState(homePage);
  return <div className="settings-page fleet-settings-page"><section className="settings-form" aria-busy={busy}>
    <header className="settings-form-heading"><p>Set the page opened by new browser sessions. Choose available browsers using the checkboxes in the sidebar.</p></header>
    <form className="settings-form-squircle fleet-detail-form fleet-text-form" onSubmit={(event) => {
      event.preventDefault();
      void onChange(() => window.beale.configureFleet({ action: 'set-browser-home-page', url: url.trim() }));
    }}>
      <div className="settings-form-control-list">
        <label className="settings-form-control-row fleet-text-field">
          <span className="settings-form-control-copy"><strong>Default page</strong><small>Used when opening a new browser page. Existing pages keep their current location.</small></span>
          <input type="url" value={url} required maxLength={2048} disabled={busy} placeholder="https://duckduckgo.com" onChange={(event) => setUrl(event.currentTarget.value)} />
        </label>
      </div>
      {error ? <p className="settings-form-error" role="alert">{error}</p> : null}
      <div className="settings-form-actions fleet-detail-actions"><button type="submit" className="primary-button" disabled={busy || url.trim() === homePage}>Save Browser Settings</button></div>
    </form>
    {browserCount === 0 ? <p className="fleet-machine-empty">No browsers were found.</p> : null}
  </section></div>;
}

function FleetAppServerDetails({ server, busy, error, restarting, onRestart, onBack, onSaved, onChange }: {
  server: FleetAppServer | undefined;
  busy: boolean;
  error: string | null;
  restarting: boolean;
  onRestart: () => Promise<void>;
  onBack: () => void;
  onSaved: (serverId: string) => void;
  onChange: (action: () => Promise<FleetState>) => Promise<boolean>;
}): JSX.Element {
  const [serverId] = useState(() => server?.id ?? crypto.randomUUID());
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
      action: 'set-app-server', serverId, name: name.trim(), url: url.trim(), operatorToken: operatorToken.trim(),
    }));
    if (saved) onSaved(serverId);
  };
  const remove = async (): Promise<void> => {
    if (!server) return;
    const removed = await onChange(() => window.beale.configureFleet({ action: 'remove-app-server', serverId: server.id }));
    if (removed) onBack();
  };
  return (
    <section className="settings-form fleet-detail-view" aria-label={server ? `Configure ${server.name}` : 'Add App Server'}>
      <header className="settings-form-heading"><p>Configure this app server on the current machine.</p></header>
      <form className="settings-form-squircle fleet-detail-form fleet-text-form" onSubmit={(event) => void save(event)}>
        <div className="settings-form-control-list">
          <label className="settings-form-control-row fleet-text-field">
            <span className="settings-form-control-copy"><strong>Name</strong></span>
            <input value={name} disabled={locked} onChange={(event) => setName(event.currentTarget.value)} placeholder="Research machine" required />
          </label>
          <label className="settings-form-control-row fleet-text-field">
            <span className="settings-form-control-copy"><strong>Tailscale HTTPS URL</strong></span>
            <input value={url} disabled={locked} onChange={(event) => { setUrl(event.currentTarget.value); setTestResult(null); }} placeholder="https://machine.tailnet.ts.net:47174" required />
          </label>
          <label className="settings-form-control-row fleet-text-field">
            <span className="settings-form-control-copy"><strong>Operator token</strong><small>Stored only on this machine. The remote server must already have Tailscale Serve enabled.</small></span>
            <input type="password" value={operatorToken} disabled={locked} onChange={(event) => { setOperatorToken(event.currentTarget.value); setTestResult(null); }} placeholder={server ? 'Saved; enter a new token to replace' : 'Token from the remote app server'} required={!server} />
          </label>
        </div>
        <div className="fleet-app-server-test-actions">
          <div className="fleet-app-server-test-buttons">
            <button type="button" className="secondary-button" disabled={locked || !url.trim() || (!server && !operatorToken.trim())} onClick={() => void test()}>{testing ? 'Testing…' : 'Test Connection'}</button>
            {server ? <FleetPairingQrCodeAction serverId={server.id} serverName={server.name} disabled={locked} /> : null}
            {server ? <button type="button" className="secondary-button" disabled={locked} onClick={() => void onRestart()}>
              <RotateCw size={14} aria-hidden="true" />{restarting ? 'Restarting…' : 'Restart App Server'}
            </button> : null}
          </div>
          {testResult ? <p className={testResult.success ? 'fleet-vm-test-success' : 'settings-form-error'} role="status">{testResult.message}</p> : null}
        </div>
        {error ? <p className="settings-form-error" role="alert">{error}</p> : null}
        {restarting ? <p className="fleet-machine-empty" role="status">Restarting app-server…</p> : null}
        <div className="settings-form-actions fleet-detail-actions">
          {server ? <button type="button" className="secondary-button" disabled={locked} onClick={() => void remove()}>Remove</button> : null}
          <button type="submit" className="primary-button" disabled={locked}>Save App Server</button>
        </div>
      </form>
    </section>
  );
}

function FleetMachineDetails({ machine, busy, error, onChange }: {
  machine: FleetMachine;
  busy: boolean;
  error: string | null;
  onChange: (action: () => Promise<FleetState>) => Promise<boolean>;
}): JSX.Element {
  const [canRun, setCanRun] = useState(machine.canRun);
  const [canClone, setCanClone] = useState(machine.canClone);
  const [guestPlatform, setGuestPlatform] = useState(machine.guestPlatform);
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

  const saveConfiguration = (): Promise<boolean> => onChange(() => window.beale.configureFleet({
      action: 'set-machine', machineId: machine.id, base: canClone && !canRun, canRun, canClone, guestPlatform, privilege,
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
    await saveConfiguration();
  };

  const clone = async (): Promise<void> => {
    if (!await saveConfiguration()) return;
    const cloned = await onChange(() => window.beale.cloneFleetVm(machine.id, cloneName.trim()));
    if (cloned) setCloneName('');
  };

  return (
    <section className="settings-form fleet-detail-view" aria-label={`Configure ${machine.name}`}>
      <header className="settings-form-heading"><p>{machine.backend} · {machine.state}{machine.owner ? ' · Reserved by a session' : ''}</p></header>
      <form className="settings-form-squircle fleet-detail-form fleet-text-form" onSubmit={(event) => void save(event)}>
        <div className="settings-form-control-list">
          <label className="settings-form-control-row">
            <span className="settings-form-control-copy"><strong>Can Run</strong><small>Allow a workspace session to run commands on this VM.</small></span>
            <input type="checkbox" checked={canRun} disabled={locked || Boolean(machine.owner)} onChange={(event) => setCanRun(event.currentTarget.checked)} />
          </label>
          <label className="settings-form-control-row">
            <span className="settings-form-control-copy"><strong>Can Clone</strong><small>Allow a workspace session to clone this stopped VM.</small></span>
            <input type="checkbox" checked={canClone} disabled={locked || Boolean(machine.owner) || machine.state !== 'stopped'} onChange={(event) => setCanClone(event.currentTarget.checked)} />
          </label>
          <label className="settings-form-control-row">
            <span className="settings-form-control-copy"><strong>Privilege</strong><small>Label this VM's manually configured privilege level.</small></span>
            <select value={privilege} disabled={locked || Boolean(machine.owner)} onChange={(event) => setPrivilege(event.currentTarget.value as FleetMachine['privilege'])}><option value="standard">Standard</option><option value="elevated">Elevated</option></select>
          </label>
          <label className="settings-form-control-row">
            <span className="settings-form-control-copy"><strong>Guest OS</strong><small>Used to format commands sent over SSH.</small></span>
            <select value={guestPlatform} disabled={locked || Boolean(machine.owner)} onChange={(event) => setGuestPlatform(event.currentTarget.value as FleetMachine['guestPlatform'])}><option value="win32">Windows</option><option value="darwin">macOS</option><option value="linux">Linux</option></select>
          </label>
          <label className="settings-form-control-row fleet-text-field">
            <span className="settings-form-control-copy"><strong>SSH host override</strong></span>
            <input value={sshHost} disabled={locked || Boolean(machine.owner)} placeholder="Auto-detect VM IP" onChange={(event) => { setSshHost(event.currentTarget.value); setTestResult(null); }} />
          </label>
          <label className="settings-form-control-row fleet-text-field">
            <span className="settings-form-control-copy"><strong>SSH user</strong><small>Beale needs this account name to connect; this does not change SSH inside the VM.</small></span>
            <input value={sshUser} disabled={locked || Boolean(machine.owner)} placeholder="Guest account name" onChange={(event) => { setSshUser(event.currentTarget.value); setTestResult(null); }} />
          </label>
          <label className="settings-form-control-row fleet-text-field">
            <span className="settings-form-control-copy"><strong>SSH identity file</strong></span>
            <input value={sshIdentityFile} disabled={locked || Boolean(machine.owner) || clearIdentity} placeholder={machine.sshIdentityConfigured ? 'Configured; enter a new path to replace' : 'Optional host path'} onChange={(event) => { setSshIdentityFile(event.currentTarget.value); setTestResult(null); }} />
          </label>
          {machine.sshIdentityConfigured ? (
            <label className="settings-form-control-row">
              <span className="settings-form-control-copy"><strong>Clear saved SSH identity file</strong></span>
              <input type="checkbox" checked={clearIdentity} disabled={locked || Boolean(machine.owner)} onChange={(event) => { setClearIdentity(event.currentTarget.checked); setTestResult(null); }} />
            </label>
          ) : null}
          <label className="settings-form-control-row fleet-text-field">
            <span className="settings-form-control-copy"><strong>SSH known-hosts file</strong><small>For Tart, Beale uses ~/.ssh/tart_known_hosts. A first connection records the VM host key there.</small></span>
            <input value={sshKnownHostsFile} disabled={locked || Boolean(machine.owner) || clearKnownHosts} placeholder={machine.sshKnownHostsConfigured ? 'Configured; enter a new path to replace' : 'Optional host path'} onChange={(event) => { setSshKnownHostsFile(event.currentTarget.value); setTestResult(null); }} />
          </label>
          {machine.sshKnownHostsConfigured ? (
            <label className="settings-form-control-row">
              <span className="settings-form-control-copy"><strong>Clear saved known-hosts file</strong></span>
              <input type="checkbox" checked={clearKnownHosts} disabled={locked || Boolean(machine.owner)} onChange={(event) => { setClearKnownHosts(event.currentTarget.checked); setTestResult(null); }} />
            </label>
          ) : null}
          {canClone ? <label className="settings-form-control-row fleet-text-field">
            <span className="settings-form-control-copy"><strong>Clone name</strong></span>
            <input value={cloneName} disabled={locked} placeholder="New VM name" onChange={(event) => setCloneName(event.currentTarget.value)} />
          </label> : null}
        </div>
        <div className="settings-form-actions fleet-vm-action-bar">
          <button type="button" className="secondary-button" disabled={locked} onClick={() => void testConnection()}>{testing ? 'Testing…' : 'Test SSH'}</button>
          {canClone ? <button type="button" className="secondary-button" disabled={locked || machine.state !== 'stopped' || !cloneName.trim()} onClick={() => void clone()}>Clone VM</button> : null}
          {machine.canRun ? <button type="button" className="secondary-button" disabled={locked || Boolean(machine.owner) || machine.state === 'unknown'} onClick={() => void onChange(() => machine.state === 'running' ? window.beale.stopFleetVm(machine.id) : window.beale.startFleetVm(machine.id))}>{machine.state === 'running' ? 'Stop VM' : 'Start VM'}</button> : null}
        </div>
        {testResult ? <p className={testResult.success ? 'fleet-vm-test-success' : 'settings-form-error'} role="status">{testResult.message}</p> : null}
        {error ? <p className="settings-form-error" role="alert">{error}</p> : null}
        <div className="settings-form-actions fleet-detail-actions">
          <button type="submit" className="primary-button" disabled={locked || Boolean(machine.owner)}>Save VM</button>
        </div>
      </form>
    </section>
  );
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
