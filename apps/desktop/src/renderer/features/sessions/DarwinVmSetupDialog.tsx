import { useState } from 'react';
import type { JSX } from 'react';
import type { DarwinVmSetupState, DarwinVmSetupUpdate } from '@shared/types';
import { Modal } from '../../app/Modal';
import { userFacingErrorMessage } from '../../lib/errors';

export function shouldOfferDarwinVmSetup(state: DarwinVmSetupState): boolean {
  return state.enabled && !state.neverPrompt && !state.prepared;
}

export async function loadOptionalDarwinVmSetup(
  load: () => Promise<DarwinVmSetupState>
): Promise<DarwinVmSetupState | null> {
  try {
    return await load();
  } catch {
    return null;
  }
}

export function DarwinVmSetupDialog({ state, busy, error, onCancel, onContinue }: {
  state: DarwinVmSetupState;
  busy: boolean;
  error: string | null;
  onCancel: () => void;
  onContinue: (update?: DarwinVmSetupUpdate) => void;
}): JSX.Element {
  const [settingUp, setSettingUp] = useState(false);
  const [checkoutRoot, setCheckoutRoot] = useState(state.checkoutRoot ?? '');
  const [guideError, setGuideError] = useState<string | null>(null);
  return (
    <Modal title="Set up Darwin VM?" className="darwin-vm-setup-dialog" closeDisabled={busy} onClose={onCancel}
      footer={settingUp ? (
        <>
          <button type="button" className="secondary-button" disabled={busy} onClick={() => onContinue()}>Set up later</button>
          <button type="button" className="primary-button" disabled={busy || !checkoutRoot.trim()}
            onClick={() => onContinue({ checkoutRoot: checkoutRoot.trim() })}>{busy ? 'Checking…' : 'Validate and continue'}</button>
        </>
      ) : (
        <>
          <button type="button" className="secondary-button" disabled={busy} onClick={() => onContinue({ neverPrompt: true })}>Never ask again</button>
          <button type="button" className="secondary-button" disabled={busy} onClick={() => onContinue()}>Not now</button>
          <button type="button" className="primary-button" disabled={busy} onClick={() => setSettingUp(true)}>Set up Darwin VM</button>
        </>
      )}>
      <p>Darwin VM provides a minimal Darwin command-line environment without a physical Apple device. Use it for low-level OS inspection; use Tart for a full macOS environment with apps and services.</p>
      {settingUp ? (
        <div className="darwin-vm-setup-steps">
          <ol>
            <li>Follow the <a href="https://github.com/jprx/darwin-vm#quick-start" onClick={(event) => {
              event.preventDefault();
              setGuideError(null);
              void window.beale.openExternalUrl('https://github.com/jprx/darwin-vm#quick-start')
                .catch((caught: unknown) => setGuideError(userFacingErrorMessage(caught)));
            }}>upstream setup guide</a> in a dedicated checkout. Record its revision and your firmware device/build selection.</li>
            <li>Prepare firmware on a Mac using the upstream prerequisites and reviewed preparation scripts. This step can require elevated privileges.</li>
            <li>Build the bundled QEMU fork on the execution host and place the prepared firmware in that checkout. A Mac used for preparation can be separate from the execution host.</li>
          </ol>
          <p>Beale connects a prepared checkout; firmware downloads and disk-image changes remain manual. The current plugin launcher uses a Unix QEMU build and serial socket: use a macOS or Linux app-server host.</p>
          {state.hostPlatform === 'win32' ? <p role="status">This app-server is running on Windows. Use a macOS or Linux app-server host to finish setup.</p> : null}
          <label className="field-label" htmlFor="darwin-vm-checkout">Checkout path on the app-server host</label>
          <input id="darwin-vm-checkout" autoFocus value={checkoutRoot} disabled={busy} onChange={(event) => setCheckoutRoot(event.target.value)} />
          <p>Validation checks files, not a successful boot or device/build compatibility. A modified root-shell guest does not establish stock iPhone behavior.</p>
        </div>
      ) : <p>Setup is guided and requires access to a Mac for firmware preparation. “Not now” skips this session; “Never ask again” applies to future sessions on this app-server.</p>}
      {state.errors.length > 0 && !error ? <p role="status">{state.errors.join(' ')}</p> : null}
      {error ? <p className="provider-keychain-access-error" role="alert">{error}</p> : null}
      {guideError ? <p className="provider-keychain-access-error" role="alert">{guideError}</p> : null}
    </Modal>
  );
}
