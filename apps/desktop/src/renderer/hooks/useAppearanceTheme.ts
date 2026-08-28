import { useCallback, useLayoutEffect, useState } from 'react';
import {
  DEFAULT_APPEARANCE_BACKGROUND,
  DEFAULT_APPEARANCE_TRANSPARENCY_PERCENTAGE,
  DEFAULT_APPEARANCE_THEME,
  readAppearanceBackground,
  readAppearanceTransparencyPercentage,
  readAppearanceTheme,
  writeAppearanceBackground,
  writeAppearanceTransparencyPercentage,
  writeAppearanceTheme,
  type AppearanceBackground,
  type AppearanceTransparencyPercentage,
  type AppearanceTheme
} from '../view-models/appearance';

function initialAppearanceTheme(): AppearanceTheme {
  if (typeof window === 'undefined') return DEFAULT_APPEARANCE_THEME;
  const theme = readAppearanceTheme(window.localStorage);
  document.documentElement.dataset.theme = theme;
  return theme;
}

export function useAppearanceTheme(): [AppearanceTheme, (theme: AppearanceTheme) => void] {
  const [theme, setTheme] = useState<AppearanceTheme>(initialAppearanceTheme);

  useLayoutEffect(() => {
    document.documentElement.dataset.theme = theme;
    writeAppearanceTheme(window.localStorage, theme);
  }, [theme]);

  const changeTheme = useCallback((nextTheme: AppearanceTheme): void => {
    setTheme(nextTheme);
  }, []);

  return [theme, changeTheme];
}

function initialAppearanceBackground(): AppearanceBackground {
  if (typeof window === 'undefined') return DEFAULT_APPEARANCE_BACKGROUND;
  const background = readAppearanceBackground(window.localStorage);
  document.documentElement.dataset.background = background;
  return background;
}

export function useAppearanceBackground(): [AppearanceBackground, (background: AppearanceBackground) => void] {
  const [background, setBackground] = useState<AppearanceBackground>(initialAppearanceBackground);

  useLayoutEffect(() => {
    document.documentElement.dataset.background = background;
    writeAppearanceBackground(window.localStorage, background);
    void window.beale.setWindowBackgroundEffect(background);
  }, [background]);

  const changeBackground = useCallback((nextBackground: AppearanceBackground): void => {
    setBackground(nextBackground);
  }, []);

  return [background, changeBackground];
}

function initialAppearanceTransparencyPercentage(): AppearanceTransparencyPercentage {
  if (typeof window === 'undefined') return DEFAULT_APPEARANCE_TRANSPARENCY_PERCENTAGE;
  return readAppearanceTransparencyPercentage(window.localStorage);
}

export function useAppearanceTransparencyPercentage(): [
  AppearanceTransparencyPercentage,
  (percentage: AppearanceTransparencyPercentage) => void
] {
  const [percentage, setPercentage] = useState<AppearanceTransparencyPercentage>(
    initialAppearanceTransparencyPercentage
  );

  useLayoutEffect(() => {
    writeAppearanceTransparencyPercentage(window.localStorage, percentage);
  }, [percentage]);

  return [percentage, setPercentage];
}
