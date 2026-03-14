export interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
}

export interface QuickButton {
  id: string;
  /** Display name; shows first 20 chars of command if empty */
  label: string;
  /** Command sent to terminal on click; automatically appends \n */
  command: string;
}

export interface SerialHistoryItem {
  id: string;
  name: string;
  portName: string;
  baudRate: number;
  dataBits: 5 | 6 | 7 | 8;
  stopBits: 1 | 2;
  parity: "none" | "odd" | "even";
  flowControl: "none" | "hardware" | "software";
}

export interface WindowBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface AppSettings {
  fontSize: number;
  fontFamily: string;
  scrollback: number;
  shellPath: string | null;
  /** Last saved main window bounds */
  windowBounds: WindowBounds | null;
  /** Default directory for session logs */
  logSavePath: string;
  /** Default log filename template, e.g. "{session}_{timestamp}" */
  logFileNameTemplate: string;
  theme: TerminalTheme;

  /** Copy on select: auto-copy selected text to clipboard; Ctrl+C consumes the key when selection exists (no ^C sent to PTY) */
  ctrlCCopy: boolean;
  /** Ctrl+V: paste clipboard content to terminal */
  ctrlVPaste: boolean;
  /** Middle-click: paste clipboard content to terminal */
  middleClickPaste: boolean;
  /** Whether to show the input bar */
  showInputBar: boolean;
  /** Quick buttons below the terminal */
  quickButtons: QuickButton[];
  /** Last used serial port configuration */
  lastSerialConfig: SerialHistoryItem | null;
  /** Recent serial port configurations for quick presets */
  recentSerialConfigs: SerialHistoryItem[];
  /** Input history for the input bar (most recent first) */
  inputHistory: string[];
  /** Whether to restore the previous session tabs on startup */
  restoreTabsOnStartup: boolean;
  /** Persisted split-pane layout state */
  paneLayout: unknown | null;
  /** Persisted workspace snapshot including tabs and pane layout */
  workspaceState: unknown | null;
}

const LEGACY_DEFAULT_THEME: TerminalTheme = {
  background: "#000000",
  foreground: "#ffffff",
  cursor: "#ffffff",
};

const DEFAULT_THEME: TerminalTheme = {
  background: "#000000",
  foreground: "#dcdcdc",
  cursor: "#ffffff",
};

export const MIN_TERMINAL_FONT_SIZE = 8;
export const MAX_TERMINAL_FONT_SIZE = 72;

export const DEFAULT_SETTINGS: AppSettings = {
  fontSize: 15,
  fontFamily: 'Consolas, "Courier New", monospace',
  scrollback: 10000,
  shellPath: null,
  windowBounds: null,
  logSavePath: "~/AuraTerm/logs",
  logFileNameTemplate: "{session}_{timestamp}",
  theme: DEFAULT_THEME,

  ctrlCCopy: true,
  ctrlVPaste: true,
  middleClickPaste: true,
  showInputBar: true,
  quickButtons: [],
  lastSerialConfig: null,
  recentSerialConfigs: [],
  inputHistory: [],
  restoreTabsOnStartup: false,
  paneLayout: null,
  workspaceState: null,
};

export const MAX_INPUT_HISTORY = 100;

function normalizeColor(value?: string | null) {
  return value?.trim().toLowerCase();
}

function shouldMigrateLegacyTheme(theme?: Partial<TerminalTheme> | null) {
  return normalizeColor(theme?.background) === LEGACY_DEFAULT_THEME.background
    && normalizeColor(theme?.foreground) === LEGACY_DEFAULT_THEME.foreground
    && normalizeColor(theme?.cursor) === LEGACY_DEFAULT_THEME.cursor;
}

export function normalizeAppSettings(value?: Partial<AppSettings> | null): AppSettings {
  const nextTheme: TerminalTheme = {
    ...DEFAULT_THEME,
    ...(value?.theme ?? {}),
  };

  if (shouldMigrateLegacyTheme(value?.theme)) {
    nextTheme.foreground = DEFAULT_THEME.foreground;
  }

  return {
    ...DEFAULT_SETTINGS,
    ...value,
    fontSize: Math.min(
      MAX_TERMINAL_FONT_SIZE,
      Math.max(MIN_TERMINAL_FONT_SIZE, value?.fontSize ?? DEFAULT_SETTINGS.fontSize),
    ),
    theme: nextTheme,
    quickButtons: value?.quickButtons ?? DEFAULT_SETTINGS.quickButtons,
    lastSerialConfig: value?.lastSerialConfig ?? DEFAULT_SETTINGS.lastSerialConfig,
    recentSerialConfigs: value?.recentSerialConfigs ?? DEFAULT_SETTINGS.recentSerialConfigs,
    inputHistory: value?.inputHistory ?? DEFAULT_SETTINGS.inputHistory,
    restoreTabsOnStartup: value?.restoreTabsOnStartup ?? DEFAULT_SETTINGS.restoreTabsOnStartup,
    paneLayout: value?.paneLayout ?? DEFAULT_SETTINGS.paneLayout,
    workspaceState: value?.workspaceState ?? DEFAULT_SETTINGS.workspaceState,
  };
}
