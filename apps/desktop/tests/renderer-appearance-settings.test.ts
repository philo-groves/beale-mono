import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { AppearanceSettingsView, settingsSectionLabel } from '../src/renderer/features/settings/SettingsModal';
import {
  APPEARANCE_THEME_STORAGE_KEY,
  readAppearanceTheme,
  writeAppearanceTheme
} from '../src/renderer/view-models/appearance';

describe('renderer appearance settings', () => {
  it('defaults invalid or missing persisted values to Dark', () => {
    expect(readAppearanceTheme({ getItem: () => null })).toBe('dark');
    expect(readAppearanceTheme({ getItem: () => 'sepia' })).toBe('dark');
    expect(readAppearanceTheme({ getItem: () => 'light' })).toBe('light');
    expect(readAppearanceTheme({ getItem: () => 'cream' })).toBe('cream');
    expect(readAppearanceTheme({ getItem: () => 'midnight' })).toBe('midnight');
  });

  it('persists the selected theme under the global appearance key', () => {
    const values = new Map<string, string>();
    writeAppearanceTheme({ setItem: (key, value) => values.set(key, value) }, 'cream');
    expect(values.get(APPEARANCE_THEME_STORAGE_KEY)).toBe('cream');
  });

  it('renders Light, Dark, Cream, and Midnight as first-class settings choices', () => {
    const html = renderToStaticMarkup(createElement(AppearanceSettingsView, {
      theme: 'dark',
      onChangeTheme: () => undefined
    }));

    expect(settingsSectionLabel('appearance')).toBe('Appearance');
    expect(html).toContain('<h2 id="appearance-theme-heading">Theme</h2>');
    expect(html).toContain('aria-label="Light theme"');
    expect(html).toMatch(/aria-label="Dark theme"[^>]*checked=""/u);
    expect(html).toContain('aria-label="Cream theme"');
    expect(html).toContain('data-appearance-theme="cream"');
    expect(html).toContain('aria-label="Midnight theme"');
    expect(html).toContain('data-appearance-theme="midnight"');
    expect(html).toMatch(/<span class="settings-form-control-copy">[\s\S]*?<span class="appearance-theme-control"><span class="appearance-theme-preview"/u);
    expect(html).not.toContain('appearance-background-heading');
    expect(html).not.toContain('Background transparency');
  });

  it('keeps the session heat gradient without an animated aurora', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');

    expect(styles).toContain(":root[data-theme='light']");
    expect(styles).toContain(":root[data-theme='cream']");
    expect(styles).toContain(":root[data-theme='midnight']");
    expect(styles).toContain('--panel: #fffaf1;');
    expect(styles).toContain('--panel: #08111f;');
    expect(styles).toContain('--appearance-gradient-start: #2a2a2a;');
    expect(styles).toContain('--appearance-gradient-middle: #1d1d1d;');
    expect(styles).toContain('--appearance-gradient-end: #151515;');
    expect(appSource).toContain('data-theme={appearanceTheme}');
    expect(appSource).not.toContain('data-background={appearanceBackground}');
    expect(styles).toMatch(/\.app-shell\s*\{[^}]*radial-gradient/u);
    expect(styles).toContain('ellipse 115% 82% at 50% 0%');
    expect(styles).toContain('--appearance-gradient-heat-color: color-mix(');
    expect(styles).toContain('var(--session-heat-window-surface) 88%');
    expect(styles).not.toContain('session-heat-aurora');
    expect(styles).not.toMatch(/\.app-shell::(?:before|after)/u);
    expect(styles).toMatch(/\.workbench\s*\{[^}]*background:\s*transparent;/u);
    expect(styles).not.toContain("data-background='");
    expect(appSource).toContain('sessionHeatPaletteForProfile(sessionHeatProfile, sessionHeatPreferences, appearanceTheme)');
  });
});
