import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import type { FleetState } from '@beale/app-server-runtime/protocol';
import { FleetPairingQrCodeDialog, FleetSettingsView } from '../src/renderer/features/settings/FleetSettingsView';
import { SettingsSidebar } from '../src/renderer/features/settings/SettingsModal';

describe('local app server remote access settings', () => {
  it('shows the MagicDNS controls in Fleet and removes Remote from Agent Settings', () => {
    const onNavigate = () => undefined;
    const sidebar = renderToStaticMarkup(createElement(SettingsSidebar, {
      collapsed: false, section: 'general', error: null,
      onChangeSection: onNavigate, onResizePointerDown: onNavigate
    }));
    expect(sidebar).not.toContain('>Remote</span>');

    const html = renderToStaticMarkup(createElement(FleetSettingsView, {
      state: {
        enabled: true, role: 'primary', machineId: 'machine-example', available: true, error: null,
        machines: [], remoteMachines: [], appServers: [], browsers: [],
        browserHomePage: 'https://example.test/', sandboxes: [], workspaceTargets: {}, primary: null,
        requiredWorkspaceIds: [], optionalWorkspaceIds: [], lastMachineByWorkspace: {}
      } satisfies FleetState,
      loadError: null,
      selection: { section: 'app-servers', itemId: 'local' },
      remoteAccessSettings: {
        enabled: true,
        magicDnsName: 'beale-mac.example.ts.net',
        localPort: 47_173,
        httpsPort: 47_174,
        publicUrl: 'https://beale-mac.example.ts.net:47174',
        status: 'configured',
        detail: null
      },
      remoteAccessBusy: false,
      onSelect: onNavigate,
      onStateChange: onNavigate,
      onRefresh: async () => undefined,
      onDetectRemoteAccess: async () => undefined,
      onSaveRemoteAccess: async () => undefined
    }));

    expect(html).toContain('Local App Server</h2>');
    expect(html).not.toContain('Publish the loopback app-server to this tailnet');
    expect(html).toContain('beale-mac.example.ts.net');
    expect(html).toContain('https://beale-mac.example.ts.net:47174');
    expect(html).toContain('Tailscale Serve');
    expect(html).toContain('MagicDNS Name');
    expect(html).toContain('Tailscale HTTPS URL');
    expect(html).toContain('readOnly=""');
    expect(html).toContain('type="submit" class="primary-button"');
    expect(html).toContain('Show QR Code');
  });

  it('shows a scannable pairing image without printing the operator token', () => {
    const html = renderToStaticMarkup(createElement(FleetPairingQrCodeDialog, {
      serverName: 'Example Server',
      dataUrl: 'data:image/png;base64,iVBORw0KGgo=',
      error: null,
      needsRemoteAccess: false,
      onClose: () => undefined
    }));
    expect(html).toContain('role="dialog"');
    expect(html).toContain('Pairing QR code for Example Server');
    expect(html).toContain('src="data:image/png;base64,iVBORw0KGgo="');
    expect(html).toContain('Scan QR Code');
    expect(html).not.toContain('synthetic-operator-token');
  });
});
