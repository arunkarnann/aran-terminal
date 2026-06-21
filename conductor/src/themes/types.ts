export interface ThemeColors {
  bg: string;
  panel: string;
  border: string;
  text: string;
  muted: string;
  primary: string;
  primaryHover: string;
  accent: string;
  running: string;
  idle: string;
  waiting: string;
  waitingText: string;
  error: string;
  success: string;
  hoverBg: string;
  selectedBg: string;
  inputBorder: string;
  inputFocusBorder: string;
  overlay: string;
  badgeBg: string;
  badgeText: string;
}

export interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
  cursorAccent: string;
  selectionBackground: string;
  black: string;
  red: string;
  green: string;
  yellow: string;
  blue: string;
  magenta: string;
  cyan: string;
  white: string;
  brightBlack: string;
  brightRed: string;
  brightGreen: string;
  brightYellow: string;
  brightBlue: string;
  brightMagenta: string;
  brightCyan: string;
  brightWhite: string;
}

export interface Theme {
  name: string;
  type: "dark" | "light";
  colors: ThemeColors;
  terminal: TerminalTheme;
}
