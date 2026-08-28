import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { AppearanceSettingsView, settingsSectionLabel } from '../src/renderer/features/settings/SettingsModal';
import {
  APPEARANCE_BACKGROUND_STORAGE_KEY,
  APPEARANCE_TRANSPARENCY_STORAGE_KEY,
  APPEARANCE_THEME_STORAGE_KEY,
  readAppearanceBackground,
  readAppearanceTransparencyPercentage,
  readAppearanceTheme,
  writeAppearanceBackground,
  writeAppearanceTransparencyPercentage,
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

  it('defaults transparency to 50% and persists supported 10% increments', () => {
    expect(readAppearanceTransparencyPercentage({ getItem: () => null })).toBe(50);
    expect(readAppearanceTransparencyPercentage({ getItem: () => '55' })).toBe(50);
    expect(readAppearanceTransparencyPercentage({ getItem: () => '10' })).toBe(10);
    expect(readAppearanceTransparencyPercentage({ getItem: () => '90' })).toBe(90);

    const values = new Map<string, string>();
    writeAppearanceTransparencyPercentage({ setItem: (key, value) => values.set(key, value) }, 70);
    expect(values.get(APPEARANCE_TRANSPARENCY_STORAGE_KEY)).toBe('70');
  });

  it('persists the selected theme under the global appearance key', () => {
    const values = new Map<string, string>();
    writeAppearanceTheme({ setItem: (key, value) => values.set(key, value) }, 'cream');
    expect(values.get(APPEARANCE_THEME_STORAGE_KEY)).toBe('cream');
  });

  it('defaults invalid or missing backgrounds to Gradient and persists valid selections', () => {
    expect(readAppearanceBackground({ getItem: () => null })).toBe('gradient');
    expect(readAppearanceBackground({ getItem: () => 'wallpaper' })).toBe('gradient');
    expect(readAppearanceBackground({ getItem: () => 'semi-transparent' })).toBe('semi-transparent');
    expect(readAppearanceBackground({ getItem: () => 'gradient' })).toBe('gradient');
    expect(readAppearanceBackground({ getItem: () => 'blur' })).toBe('blur');

    const values = new Map<string, string>();
    writeAppearanceBackground({ setItem: (key, value) => values.set(key, value) }, 'blur');
    expect(values.get(APPEARANCE_BACKGROUND_STORAGE_KEY)).toBe('blur');
  });

  it('renders Light, Dark, Cream, and Midnight as first-class settings choices', () => {
    const html = renderToStaticMarkup(createElement(AppearanceSettingsView, {
      background: 'solid',
      transparencyPercentage: 50,
      theme: 'dark',
      onChangeBackground: () => undefined,
      onChangeTransparencyPercentage: () => undefined,
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
    expect(html).toContain('<h2 id="appearance-background-heading">Background</h2>');
    expect(html).toMatch(/aria-label="Solid background"[^>]*checked=""/u);
    expect(html).toContain('aria-label="Semi-Transparent background"');
    expect(html).toContain('data-appearance-background="semi-transparent"');
    expect(html).toContain('aria-label="Background transparency"');
    expect(html).toMatch(/aria-label="Background transparency"[\s\S]*?<option value="10">10%<\/option>[\s\S]*?<option value="50" selected="">50%<\/option>[\s\S]*?<option value="90">90%<\/option>/u);
    expect(html).toContain('aria-label="Gradient background"');
    expect(html).toContain('aria-label="Blur background"');
  });

  it('defines adaptive Light and Cream token sets and wires the active theme to the app shell', () => {
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    const appSource = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    const mainSource = readFileSync(new URL('../src/main/index.ts', import.meta.url), 'utf8');
    const preloadSource = readFileSync(new URL('../src/preload/index.ts', import.meta.url), 'utf8');

    expect(styles).toContain(":root[data-theme='light']");
    expect(styles).toContain(":root[data-theme='cream']");
    expect(styles).toContain(":root[data-theme='midnight']");
    expect(styles).toContain('--panel: #fffaf1;');
    expect(styles).toContain('--panel: #08111f;');
    expect(styles).toContain('--appearance-gradient-start: #2a2a2a;');
    expect(styles).toContain('--appearance-gradient-middle: #1d1d1d;');
    expect(styles).toContain('--appearance-gradient-end: #151515;');
    expect(appSource).toContain('data-theme={appearanceTheme}');
    expect(appSource).toContain('data-background={appearanceBackground}');
    expect(styles).toContain(".app-shell[data-background='semi-transparent']");
    expect(styles).toContain(".app-shell[data-background='gradient']");
    expect(styles).toContain(".app-shell[data-background='blur']");
    expect(styles).toContain('backdrop-filter: blur(32px) saturate(135%);');
    expect(styles).toContain('var(--appearance-background-opacity, 50%)');
    expect(styles).toMatch(/data-background='gradient'[^}]*radial-gradient/u);
    expect(styles).toContain('ellipse 115% 82% at 50% 0%');
    expect(styles).toContain('--appearance-gradient-heat-color: color-mix(');
    expect(styles).toContain('var(--session-heat-window-surface) 88%');
    expect(styles).toMatch(/data-background='semi-transparent'\] \.workbench,[\s\S]*?background:\s*transparent;/u);
    expect(preloadSource).toContain('ipcRenderer.invoke(IPC_CHANNELS.setWindowBackgroundEffect, effect)');
    expect(mainSource).toContain("window.setVibrancy(blurEnabled ? 'under-window' : null)");
    expect(mainSource).toContain("window.setBackgroundMaterial(blurEnabled ? 'acrylic' : 'none')");
    expect(appSource).toContain('sessionHeatPaletteForProfile(sessionHeatProfile, sessionHeatPreferences, appearanceTheme)');
  });
});
