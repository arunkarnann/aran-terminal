import { createContext, useCallback, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import type { Theme } from "./types";
import {
  BUILT_IN_THEMES,
  applyThemeToRoot,
  DEFAULT_THEME_NAME,
  getThemeByName,
} from "./index";

const STORAGE_KEY = "conductor-theme";

interface ThemeContextValue {
  theme: Theme;
  themeName: string;
  availableThemes: Theme[];
  setTheme: (name: string) => void;
}

export const ThemeContext = createContext<ThemeContextValue | null>(null);

function loadPersistedTheme(): string {
  try {
    return localStorage.getItem(STORAGE_KEY) ?? DEFAULT_THEME_NAME;
  } catch {
    return DEFAULT_THEME_NAME;
  }
}

function persistTheme(name: string) {
  try {
    localStorage.setItem(STORAGE_KEY, name);
  } catch {
    /* noop */
  }
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [themeName, setThemeName] = useState(loadPersistedTheme);

  const theme = useMemo(() => {
    return getThemeByName(themeName) ?? BUILT_IN_THEMES[0];
  }, [themeName]);

  const setTheme = useCallback((name: string) => {
    setThemeName(name);
    persistTheme(name);
  }, []);

  useEffect(() => {
    applyThemeToRoot(theme, document.documentElement);
  }, [theme]);

  const value = useMemo(
    () => ({ theme, themeName, availableThemes: BUILT_IN_THEMES, setTheme }),
    [theme, themeName, setTheme],
  );

  return (
    <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
  );
}
