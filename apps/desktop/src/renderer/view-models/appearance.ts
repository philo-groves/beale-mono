import type { WindowBackgroundEffect } from '@shared/types';

export type AppearanceTheme = 'light' | 'dark' | 'cream' | 'midnight';
export type AppearanceBackground = WindowBackgroundEffect;
export const APPEARANCE_TRANSPARENCY_PERCENTAGES = [10, 20, 30, 40, 50, 60, 70, 80, 90] as const;
export type AppearanceTransparencyPercentage = typeof APPEARANCE_TRANSPARENCY_PERCENTAGES[number];

export const APPEARANCE_THEMES: readonly AppearanceTheme[] = ['light', 'dark', 'cream', 'midnight'];
export const DEFAULT_APPEARANCE_THEME: AppearanceTheme = 'dark';
export const APPEARANCE_THEME_STORAGE_KEY = 'beale.appearanceTheme';
export const APPEARANCE_BACKGROUNDS: readonly AppearanceBackground[] = [
  'solid',
  'semi-transparent',
  'gradient',
  'blur'
];
export const DEFAULT_APPEARANCE_BACKGROUND: AppearanceBackground = 'gradient';
export const APPEARANCE_BACKGROUND_STORAGE_KEY = 'beale.appearanceBackground';
export const DEFAULT_APPEARANCE_TRANSPARENCY_PERCENTAGE: AppearanceTransparencyPercentage = 50;
export const APPEARANCE_TRANSPARENCY_STORAGE_KEY = 'beale.appearanceTransparencyPercentage';

export function normalizeAppearanceTheme(value: unknown): AppearanceTheme {
  return typeof value === 'string' && APPEARANCE_THEMES.includes(value as AppearanceTheme)
    ? value as AppearanceTheme
    : DEFAULT_APPEARANCE_THEME;
}

export function readAppearanceTheme(storage: Pick<Storage, 'getItem'>): AppearanceTheme {
  try {
    return normalizeAppearanceTheme(storage.getItem(APPEARANCE_THEME_STORAGE_KEY));
  } catch {
    return DEFAULT_APPEARANCE_THEME;
  }
}

export function writeAppearanceTheme(
  storage: Pick<Storage, 'setItem'>,
  theme: AppearanceTheme
): void {
  try {
    storage.setItem(APPEARANCE_THEME_STORAGE_KEY, theme);
  } catch {
    // A renderer with unavailable storage can still use the theme for its current lifetime.
  }
}

export function normalizeAppearanceBackground(value: unknown): AppearanceBackground {
  return typeof value === 'string' && APPEARANCE_BACKGROUNDS.includes(value as AppearanceBackground)
    ? value as AppearanceBackground
    : DEFAULT_APPEARANCE_BACKGROUND;
}

export function readAppearanceBackground(storage: Pick<Storage, 'getItem'>): AppearanceBackground {
  try {
    return normalizeAppearanceBackground(storage.getItem(APPEARANCE_BACKGROUND_STORAGE_KEY));
  } catch {
    return DEFAULT_APPEARANCE_BACKGROUND;
  }
}

export function writeAppearanceBackground(
  storage: Pick<Storage, 'setItem'>,
  background: AppearanceBackground
): void {
  try {
    storage.setItem(APPEARANCE_BACKGROUND_STORAGE_KEY, background);
  } catch {
    // A renderer with unavailable storage can still use the background for its current lifetime.
  }
}

export function normalizeAppearanceTransparencyPercentage(value: unknown): AppearanceTransparencyPercentage {
  const numericValue = typeof value === 'string' && value.trim() !== '' ? Number(value) : value;
  return typeof numericValue === 'number'
    && APPEARANCE_TRANSPARENCY_PERCENTAGES.includes(numericValue as AppearanceTransparencyPercentage)
    ? numericValue as AppearanceTransparencyPercentage
    : DEFAULT_APPEARANCE_TRANSPARENCY_PERCENTAGE;
}

export function readAppearanceTransparencyPercentage(
  storage: Pick<Storage, 'getItem'>
): AppearanceTransparencyPercentage {
  try {
    return normalizeAppearanceTransparencyPercentage(storage.getItem(APPEARANCE_TRANSPARENCY_STORAGE_KEY));
  } catch {
    return DEFAULT_APPEARANCE_TRANSPARENCY_PERCENTAGE;
  }
}

export function writeAppearanceTransparencyPercentage(
  storage: Pick<Storage, 'setItem'>,
  percentage: AppearanceTransparencyPercentage
): void {
  try {
    storage.setItem(APPEARANCE_TRANSPARENCY_STORAGE_KEY, String(percentage));
  } catch {
    // A renderer with unavailable storage can still use the percentage for its current lifetime.
  }
}
