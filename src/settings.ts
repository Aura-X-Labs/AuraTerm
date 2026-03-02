export interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
}

export interface AppSettings {
  fontSize: number;
  fontFamily: string;
  scrollback: number;
  shellPath: string | null;
  theme: TerminalTheme;
}

export const DEFAULT_SETTINGS: AppSettings = {
  fontSize: 14,
  fontFamily: 'Consolas, "Courier New", monospace',
  scrollback: 1000,
  shellPath: null,
  theme: {
    background: "#000000",
    foreground: "#ffffff",
    cursor: "#ffffff",
  },
};
