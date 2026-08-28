import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import {
  RemoteAccessSettingsView,
  settingsSectionLabel
} from '../src/renderer/features/settings/SettingsModal';

describe('remote access settings', () => {
  it('renders the configured MagicDNS HTTPS endpoint', () => {
    const html = renderToStaticMarkup(createElement(RemoteAccessSettingsView, {
      settings: {
        enabled: true,
        magicDnsName: 'beale-mac.example.ts.net',
        localPort: 47_173,
        httpsPort: 47_174,
        publicUrl: 'https://beale-mac.example.ts.net:47174',
        status: 'configured',
        detail: null
      },
      busy: false,
      onDetect: async () => undefined,
      onSave: async () => undefined
    }));

    expect(settingsSectionLabel('remote')).toBe('Remote');
    expect(html).toContain('iPhone Remote Access');
    expect(html).toContain('beale-mac.example.ts.net');
    expect(html).toContain('https://beale-mac.example.ts.net:47174');
    expect(html).toContain('Tailscale Serve');
  });
});
