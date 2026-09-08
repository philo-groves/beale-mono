import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import {
  DarwinVmSetupDialog,
  loadOptionalDarwinVmSetup,
  shouldOfferDarwinVmSetup
} from '../src/renderer/features/sessions/DarwinVmSetupDialog';
import type { DarwinVmSetupState } from '@shared/types';

const state: DarwinVmSetupState = {
  enabled: true, neverPrompt: false, prepared: false, checkoutRoot: null, errors: [], hostPlatform: 'darwin'
};

describe('Darwin VM session onboarding', () => {
  it('offers setup only for an enabled plugin without a prepared checkout or permanent decline', () => {
    expect(shouldOfferDarwinVmSetup(state)).toBe(true);
    expect(shouldOfferDarwinVmSetup({ ...state, enabled: false })).toBe(false);
    expect(shouldOfferDarwinVmSetup({ ...state, neverPrompt: true })).toBe(false);
    expect(shouldOfferDarwinVmSetup({ ...state, prepared: true })).toBe(false);
    expect(shouldOfferDarwinVmSetup({ ...state, checkoutRoot: '/example/missing', errors: ['Missing artifact.'] })).toBe(true);
  });

  it('does not block research startup when optional setup discovery is unavailable', async () => {
    await expect(loadOptionalDarwinVmSetup(async () => state)).resolves.toEqual(state);
    await expect(loadOptionalDarwinVmSetup(async () => {
      throw new Error('Unsupported app-server client command: harness plugin-darwin-vm-get');
    })).resolves.toBeNull();
  });

  it('renders all three choices, setup boundaries, squircle class, and retryable errors', () => {
    const html = renderToStaticMarkup(createElement(DarwinVmSetupDialog, {
      state, busy: false, error: 'Example validation failure.', onCancel: () => {}, onContinue: () => {}
    }));
    expect(html).toContain('darwin-vm-setup-dialog');
    expect(html).toContain('role="dialog"');
    expect(html).toContain('Never ask again');
    expect(html).toContain('Not now');
    expect(html).toContain('Set up Darwin VM');
    expect(html).toContain('Tart');
    expect(html).toContain('Mac for firmware preparation');
    expect(html).toContain('role="alert"');
    expect(html).toContain('Example validation failure.');
  });
});
