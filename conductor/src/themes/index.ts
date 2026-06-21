import type { Theme, TerminalTheme } from "./types";
import dracula from "./dracula.json";
import oneDark from "./one-dark.json";
import nord from "./nord.json";
import catppuccinMocha from "./catppuccin-mocha.json";
import tokyoNightStorm from "./tokyo-night-storm.json";
import materialPalenight from "./material-palenight.json";
import githubDark from "./github-dark.json";
import githubLight from "./github-light.json";

export type { Theme, ThemeColors, TerminalTheme } from "./types";

export const BUILT_IN_THEMES: Theme[] = [
  dracula as Theme,
  oneDark as Theme,
  nord as Theme,
  catppuccinMocha as Theme,
  tokyoNightStorm as Theme,
  materialPalenight as Theme,
  githubDark as Theme,
  githubLight as Theme,
];

export function getThemeByName(name: string): Theme | undefined {
  return BUILT_IN_THEMES.find((t) => t.name === name);
}

export const DEFAULT_THEME_NAME = "Dracula";

/** Map a Theme's color tokens to CSS custom properties on the given element. */
export function applyThemeToRoot(theme: Theme, root: HTMLElement) {
  const { colors } = theme;
  for (const [key, value] of Object.entries(colors)) {
    root.style.setProperty(`--${key}`, value);
  }
}

/** Extract the xterm.js theme from a Theme.terminal. */
export function toXtermTheme(t: TerminalTheme): Record<string, string> {
  return {
    background: t.background,
    foreground: t.foreground,
    cursor: t.cursor,
    cursorAccent: t.cursorAccent,
    selectionBackground: t.selectionBackground,
    black: t.black,
    red: t.red,
    green: t.green,
    yellow: t.yellow,
    blue: t.blue,
    magenta: t.magenta,
    cyan: t.cyan,
    white: t.white,
    brightBlack: t.brightBlack,
    brightRed: t.brightRed,
    brightGreen: t.brightGreen,
    brightYellow: t.brightYellow,
    brightBlue: t.brightBlue,
    brightMagenta: t.brightMagenta,
    brightCyan: t.brightCyan,
    brightWhite: t.brightWhite,
  };
}
