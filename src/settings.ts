import { normalizeBookmarkPath } from "./bookmarks";
import type { AppLanguage } from "./i18n";
import type { SerialTransport } from "./types";

export interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
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

export interface TerminalThemePreset {
  id: string;
  label: string;
  description: string;
  theme: TerminalTheme;
}

export type ThemeAppearance = "light" | "dark";

export interface DerivedUiTheme {
  appearance: ThemeAppearance;
  variables: Record<string, string>;
}

export type UiThemeMode = "follow-terminal" | ThemeAppearance;

/**
 * Terminal renderer selection.
 * - "auto": attach the WebGL (GPU) renderer to the visible terminal and fall
 *   back to the default DOM renderer if WebGL is unavailable or its context is
 *   lost (recommended; safe across the different Tauri webviews).
 * - "webgl": same as auto but never used as a soft preference — still falls
 *   back on hard failure since a broken renderer would otherwise blank the view.
 * - "dom": never load WebGL; always use the default DOM renderer.
 */
export type RendererMode = "auto" | "webgl" | "dom";

export type AiProvider = "anthropic" | "openai-compatible";

/**
 * AI assistant configuration. The API key deliberately lives outside this
 * structure (and outside settings.json): it is stored encrypted under the
 * device-local key via the `ai_set_api_key` command and never syncs.
 */
export interface AiConfig {
  enabled: boolean;
  provider: AiProvider;
  /** Required for openai-compatible (e.g. https://api.deepseek.com/v1); null for anthropic means the official endpoint. */
  baseUrl: string | null;
  model: string;
  /** Per-reply output token cap; null falls back to the backend default (4096). */
  maxTokens: number | null;
}

export const DEFAULT_AI_MODELS: Record<AiProvider, string> = {
  anthropic: "claude-opus-4-8",
  "openai-compatible": "",
};

export const DEFAULT_AI_CONFIG: AiConfig = {
  enabled: false,
  provider: "anthropic",
  baseUrl: null,
  model: DEFAULT_AI_MODELS.anthropic,
  maxTokens: null,
};

/**
 * Live Relay provider policy (design docs/plans/live-sync-design.md §5.8):
 * whether this machine may be reached from the account's other AuraTerm
 * devices, and what they may do. Every gate defaults closed except plain
 * attach. A summary is mirrored into the cloud bridge
 * (`cloud_bridge_set_relay_policy`) and reported with presence pings; the
 * local process stays the authority on every actual request.
 */
export interface LiveRelaySettings {
  /** Master gate; off means this device is unreachable via Live Relay. */
  enabled: boolean;
  /** Allow attaching to sessions this device already shares. */
  allowAttach: boolean;
  /** Allow a peer device to open a fresh local shell here. */
  allowOpenShell: boolean;
  /** Allow a peer device to open one of this machine's serial ports. */
  allowOpenSerial: boolean;
  /** Allow a peer device to start SSH/Telnet from this machine's bookmarks. */
  allowOpenBookmark: boolean;
  /** Ask on this machine before admitting each peer. */
  requireApproval: boolean;
  /** Allow peer input at all (still clamped by the global `allowRemoteSend` gate). */
  allowRemoteInput: boolean;
  maxConcurrentPeers: number;
  /** Take control back from a peer that stops typing for this long; 0
   *  disables. Silent viewers are left alone. */
  idleTimeoutMinutes: number;
}

export const DEFAULT_LIVE_RELAY: LiveRelaySettings = {
  enabled: false,
  allowAttach: true,
  allowOpenShell: false,
  allowOpenSerial: false,
  allowOpenBookmark: false,
  requireApproval: true,
  allowRemoteInput: true,
  maxConcurrentPeers: 3,
  idleTimeoutMinutes: 30,
};

export const TERMINAL_THEME_KEYS: Array<keyof TerminalTheme> = [
  "background",
  "foreground",
  "cursor",
  "selectionBackground",
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite",
];

export interface QuickButton {
  id: string;
  /** Display name; shows first 20 chars of command if empty */
  label: string;
  /** Command sent to terminal on click; automatically appends \n */
  command: string;
  /** Named toolbar. Buttons from different toolbars can be switched per session. */
  toolbar?: string;
  /** Optional visual group within a toolbar. */
  group?: string;
  /** Host globs this snippet is visible for. Empty means all sessions. */
  hosts?: string[];
  /** Saved connection groups this snippet is visible for. */
  sessionGroups?: string[];
  /** `line` appends Enter; `raw` sends decoded control characters as-is. */
  sendMode?: "line" | "raw";
}

export type OutputRuleScope = "global" | "hosts";

export interface OutputRule {
  id: string;
  name: string;
  enabled: boolean;
  pattern: string;
  isRegex: boolean;
  caseSensitive: boolean;
  scope: OutputRuleScope;
  hosts: string[];
  foreground?: string;
  background?: string;
  bell: boolean;
  notify: boolean;
  autoResponse?: string;
  cooldownMs: number;
}

export interface SerialHistoryItem {
  id: string;
  name: string;
  portName: string;
  /** Absent means a local port, so history saved before network serial existed
   *  keeps loading. */
  transport?: SerialTransport;
  host?: string;
  netPort?: number;
  adoptServerParams?: boolean;
  autoReconnect?: boolean;
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
  /** Interface language; "system" follows the OS/browser language. */
  language: AppLanguage;
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
  /** Whether UI follows terminal theme appearance or uses a fixed light/dark style */
  uiThemeMode: UiThemeMode;
  /** Terminal renderer backend. See {@link RendererMode}. */
  rendererMode: RendererMode;

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
  /** Bookmark groups the user created explicitly. Folders are otherwise derived
   *  from each bookmark's group path, so a freshly created (still empty) one
   *  would disappear; this list is what keeps it on screen. */
  bookmarkGroups: string[];
  /** Shared highlight and trigger rules applied to terminal output. */
  outputRules: OutputRule[];
  /** Automatically reveal the SFTP browser after an SSH connection succeeds. */
  autoOpenSftp: boolean;
  /** Keep the Cloud Console share slot on the active session, without a
   * per-session confirmation, until switched off. The share policy follows
   * `allowRemoteSend` (read-write when allowed, read-only otherwise). */
  autoShareToCloud: boolean;
  /** Global Cloud Console TX gate. On (default): shares may grant remote
   * input per their TX policy. Off: the cloud can only observe — new shares
   * are clamped to read-only and remote INPUT is dropped by the bridge. */
  allowRemoteSend: boolean;
  /** Destination directory for files received through inline Zmodem. */
  zmodemDownloadPath: string;
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
  /** Master password hash (Argon2 format) - managed by backend */
  masterPasswordHash?: string | null;
  /** Master password salt (Base64 encoded) - managed by backend */
  masterPasswordSalt?: string | null;
  /** Whether credentials have been initialized with encryption */
  credentialsInitialized?: boolean;
  /** When a master password is set, cache it in the OS keychain for auto-unlock (macOS/Windows) */
  rememberMasterPassword?: boolean;
  /** AI assistant configuration (API key excluded; stored encrypted separately) */
  aiConfig: AiConfig;
  /** Live Relay provider policy; defaults keep the device unreachable. */
  liveRelay: LiveRelaySettings;
}

const LEGACY_DEFAULT_THEME: TerminalTheme = {
  background: "#000000",
  foreground: "#ffffff",
  cursor: "#ffffff",
  selectionBackground: "#264f78",
  black: "#2d2e2e",
  red: "#f14c4c",
  green: "#23d18b",
  yellow: "#f5f543",
  blue: "#3b8eea",
  magenta: "#d670d6",
  cyan: "#29b8db",
  white: "#e5e5e5",
  brightBlack: "#666666",
  brightRed: "#f14c4c",
  brightGreen: "#23d18b",
  brightYellow: "#f5f543",
  brightBlue: "#3b8eea",
  brightMagenta: "#d670d6",
  brightCyan: "#29b8db",
  brightWhite: "#ffffff",
};

const DEFAULT_THEME: TerminalTheme = {
  background: "#000000",
  foreground: "#dcdcdc",
  cursor: "#ffffff",
  selectionBackground: "#264f78",
  black: "#1f252d",
  red: "#c35b65",
  green: "#7fb069",
  yellow: "#d0b26f",
  blue: "#6ca0d8",
  magenta: "#a889d8",
  cyan: "#5fb3b3",
  white: "#dcdcdc",
  brightBlack: "#5c6370",
  brightRed: "#e06c75",
  brightGreen: "#98c379",
  brightYellow: "#e5c07b",
  brightBlue: "#61afef",
  brightMagenta: "#c678dd",
  brightCyan: "#56b6c2",
  brightWhite: "#ffffff",
};

export const TERMINAL_THEME_PRESETS: TerminalThemePreset[] = [
  {
    id: "aura-dark",
    label: "Aura Dark",
    description: "Default dark palette with softer ANSI colors for long terminal sessions.",
    theme: DEFAULT_THEME,
  },
  {
    id: "nord-frost",
    label: "Nord Frost",
    description: "Cool, low-contrast palette with restrained reds and blues.",
    theme: {
      background: "#2e3440",
      foreground: "#d8dee9",
      cursor: "#eceff4",
      selectionBackground: "#434c5e",
      black: "#3b4252",
      red: "#bf616a",
      green: "#a3be8c",
      yellow: "#ebcb8b",
      blue: "#81a1c1",
      magenta: "#b48ead",
      cyan: "#88c0d0",
      white: "#e5e9f0",
      brightBlack: "#4c566a",
      brightRed: "#d57780",
      brightGreen: "#b1c89b",
      brightYellow: "#f0d399",
      brightBlue: "#8baed0",
      brightMagenta: "#c19bb7",
      brightCyan: "#8fbcbb",
      brightWhite: "#eceff4",
    },
  },
  {
    id: "shanghai-night",
    label: "Shanghai Night",
    description: "Balanced deep charcoal with low-contrast amber and muted garden accents.",
    theme: {
      background: "#0d0d0d",
      foreground: "#d1c7bc",
      cursor: "#d4a76a",
      selectionBackground: "#2a2826",
      black: "#1a1a1a",
      red: "#b26354",
      green: "#6a8f76",
      yellow: "#b58e59",
      blue: "#5d7896",
      magenta: "#9071a1",
      cyan: "#4b8c88",
      white: "#b5aca1",
      brightBlack: "#4a443d",
      brightRed: "#c27c6e",
      brightGreen: "#82a68c",
      brightYellow: "#c4a376",
      brightBlue: "#7a95b3",
      brightMagenta: "#a88bb8",
      brightCyan: "#67aca8",
      brightWhite: "#e8e1d7",
    },
  },
  {
    id: "gruvbox-soft",
    label: "Gruvbox Soft",
    description: "Warm, low-glare palette that reduces harsh reds and bright whites.",
    theme: {
      background: "#32302f",
      foreground: "#ebdbb2",
      cursor: "#fabd2f",
      selectionBackground: "#504945",
      black: "#3c3836",
      red: "#cc241d",
      green: "#98971a",
      yellow: "#d79921",
      blue: "#458588",
      magenta: "#b16286",
      cyan: "#689d6a",
      white: "#a89984",
      brightBlack: "#7c6f64",
      brightRed: "#fb4934",
      brightGreen: "#b8bb26",
      brightYellow: "#fabd2f",
      brightBlue: "#83a598",
      brightMagenta: "#d3869b",
      brightCyan: "#8ec07c",
      brightWhite: "#fbf1c7",
    },
  },
  {
    id: "shanghai-dawn",
    label: "Shanghai Dawn",
    description: "Warm paper-soft light palette inspired by classic parchment and muted ink.",
    theme: {
      background: "#f7f1e6",
      foreground: "#3d3935",
      cursor: "#bc8c5a",
      selectionBackground: "#e5dccc",
      black: "#3d3935",
      red: "#a65242",
      green: "#597a63",
      yellow: "#9e7441",
      blue: "#4a637d",
      magenta: "#7b5b8c",
      cyan: "#3e706d",
      white: "#d8cfc0",
      brightBlack: "#7d7469",
      brightRed: "#bf6b5b",
      brightGreen: "#70917a",
      brightYellow: "#b88d57",
      brightBlue: "#63809c",
      brightMagenta: "#9476a6",
      brightCyan: "#578c89",
      brightWhite: "#fff9f0",
    },
  },  
  {
    id: "paper-light",
    label: "Paper Light",
    description: "Clean light terminal palette with softened reds and calm contrast for daytime use.",
    theme: {
      background: "#f6f3ea",
      foreground: "#3a342c",
      cursor: "#7c5c2f",
      selectionBackground: "#d9e5f5",
      black: "#3b332b",
      red: "#c46a5a",
      green: "#6f8f5f",
      yellow: "#b08b45",
      blue: "#5a85b0",
      magenta: "#9a6ba2",
      cyan: "#4f8f93",
      white: "#e9dfcf",
      brightBlack: "#75685a",
      brightRed: "#d98272",
      brightGreen: "#84a871",
      brightYellow: "#c8a45f",
      brightBlue: "#6a98c9",
      brightMagenta: "#ae7fb6",
      brightCyan: "#62a5aa",
      brightWhite: "#fffdf7",
    },
  },
  {
    id: "canvas-office",
    label: "Canvas Office",
    description: "Bright neutral office palette with gentle blues and lower-saturation alerts.",
    theme: {
      background: "#f7f8fb",
      foreground: "#2b3441",
      cursor: "#47658a",
      selectionBackground: "#dbe7f5",
      black: "#4a5563",
      red: "#bb6d68",
      green: "#6f9173",
      yellow: "#b49658",
      blue: "#6287b5",
      magenta: "#9074ad",
      cyan: "#5e96a1",
      white: "#e9edf4",
      brightBlack: "#6c7888",
      brightRed: "#cf7f79",
      brightGreen: "#82a684",
      brightYellow: "#c7aa69",
      brightBlue: "#7499c8",
      brightMagenta: "#a288bf",
      brightCyan: "#71aab3",
      brightWhite: "#ffffff",
    },
  },
  {
    id: "ledger-day",
    label: "Ledger Day",
    description: "Warm paper-like preset tuned for spreadsheets, logs, and bright office displays.",
    theme: {
      background: "#f4f0e8",
      foreground: "#3a362f",
      cursor: "#7f6542",
      selectionBackground: "#d8e3f1",
      black: "#4b4338",
      red: "#be7263",
      green: "#708b63",
      yellow: "#b08a4c",
      blue: "#5f84ae",
      magenta: "#936fa8",
      cyan: "#5d9195",
      white: "#e8dfd2",
      brightBlack: "#706557",
      brightRed: "#d38576",
      brightGreen: "#86a177",
      brightYellow: "#c79f5d",
      brightBlue: "#7298c2",
      brightMagenta: "#a784bb",
      brightCyan: "#70a5a9",
      brightWhite: "#fffdf9",
    },
  },
];

const UI_LIGHT_THEME: TerminalTheme = {
  background: "#f5f7fb",
  foreground: "#243041",
  cursor: "#5274a1",
  selectionBackground: "#dbe6f4",
  black: "#4d5968",
  red: "#b96b66",
  green: "#678868",
  yellow: "#a8884d",
  blue: "#567fad",
  magenta: "#8a70af",
  cyan: "#4f8f98",
  white: "#e8edf5",
  brightBlack: "#6d7987",
  brightRed: "#cc7d78",
  brightGreen: "#7c9d7d",
  brightYellow: "#be9d60",
  brightBlue: "#6a93c1",
  brightMagenta: "#9d84c1",
  brightCyan: "#67a4ac",
  brightWhite: "#ffffff",
};

const UI_DARK_THEME: TerminalTheme = {
  background: "#161b23",
  foreground: "#d5dbe5",
  cursor: "#8fb2d9",
  selectionBackground: "#2c3b52",
  black: "#1b2230",
  red: "#c06c76",
  green: "#7daa78",
  yellow: "#c5a364",
  blue: "#6d98cb",
  magenta: "#a687c7",
  cyan: "#5fa8ad",
  white: "#dbe2eb",
  brightBlack: "#5f6978",
  brightRed: "#d9818a",
  brightGreen: "#95bd8d",
  brightYellow: "#dab779",
  brightBlue: "#86b0e1",
  brightMagenta: "#bda0dd",
  brightCyan: "#76bcc0",
  brightWhite: "#ffffff",
};

export const MIN_TERMINAL_FONT_SIZE = 8;
export const MAX_TERMINAL_FONT_SIZE = 72;

export const DEFAULT_SETTINGS: AppSettings = {
  language: "system",
  fontSize: 15,
  fontFamily: 'Consolas, "Courier New", monospace',
  scrollback: 10000,
  shellPath: null,
  windowBounds: null,
  logSavePath: "~/AuraTerm/logs",
  logFileNameTemplate: "{session}_{timestamp}",
  theme: DEFAULT_THEME,
  uiThemeMode: "follow-terminal",
  rendererMode: "auto",

  ctrlCCopy: true,
  ctrlVPaste: true,
  middleClickPaste: true,
  showInputBar: true,
  quickButtons: [],
  bookmarkGroups: [],
  outputRules: [],
  autoOpenSftp: false,
  autoShareToCloud: false,
  allowRemoteSend: true,
  zmodemDownloadPath: "~/AuraTerm/downloads",
  lastSerialConfig: null,
  recentSerialConfigs: [],
  inputHistory: [],
  restoreTabsOnStartup: false,
  paneLayout: null,
  workspaceState: null,
  masterPasswordHash: null,
  masterPasswordSalt: null,
  credentialsInitialized: false,
  rememberMasterPassword: false,
  aiConfig: DEFAULT_AI_CONFIG,
  liveRelay: DEFAULT_LIVE_RELAY,
};

export const MAX_INPUT_HISTORY = 100;

function normalizeColor(value?: string | null) {
  return value?.trim().toLowerCase();
}

function normalizeLanguage(value?: string | null): AppLanguage {
  return value === "en" || value === "zh-CN" || value === "system"
    ? value
    : DEFAULT_SETTINGS.language;
}

function normalizeUiThemeMode(value?: string | null): UiThemeMode {
  return value === "light" || value === "dark" || value === "follow-terminal"
    ? value
    : DEFAULT_SETTINGS.uiThemeMode;
}

function normalizeRendererMode(value?: string | null): RendererMode {
  return value === "auto" || value === "webgl" || value === "dom"
    ? value
    : DEFAULT_SETTINGS.rendererMode;
}

function normalizeAiProvider(value?: string | null): AiProvider {
  return value === "anthropic" || value === "openai-compatible"
    ? value
    : DEFAULT_AI_CONFIG.provider;
}

export function normalizeAiConfig(value?: Partial<AiConfig> | null): AiConfig {
  const provider = normalizeAiProvider(value?.provider);
  const maxTokens = typeof value?.maxTokens === "number" && Number.isFinite(value.maxTokens)
    ? Math.max(1, Math.floor(value.maxTokens))
    : null;
  return {
    enabled: value?.enabled === true,
    provider,
    baseUrl: value?.baseUrl?.trim() || null,
    model: value?.model?.trim() || DEFAULT_AI_MODELS[provider],
    maxTokens,
  };
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
    language: normalizeLanguage(value?.language),
    fontSize: Math.min(
      MAX_TERMINAL_FONT_SIZE,
      Math.max(MIN_TERMINAL_FONT_SIZE, value?.fontSize ?? DEFAULT_SETTINGS.fontSize),
    ),
    theme: nextTheme,
    uiThemeMode: normalizeUiThemeMode(value?.uiThemeMode),
    rendererMode: normalizeRendererMode(value?.rendererMode),
    bookmarkGroups: Array.isArray(value?.bookmarkGroups)
      ? [...new Set(value.bookmarkGroups
        .filter((group): group is string => typeof group === "string")
        .map((group) => normalizeBookmarkPath(group).join("/"))
        .filter(Boolean))]
      : [...DEFAULT_SETTINGS.bookmarkGroups],
    quickButtons: (value?.quickButtons ?? DEFAULT_SETTINGS.quickButtons).map((button) => ({
      ...button,
      toolbar: button.toolbar?.trim() || "Default",
      group: button.group?.trim() || "General",
      hosts: Array.isArray(button.hosts) ? button.hosts.filter((host) => typeof host === "string" && host.trim()) : [],
      sessionGroups: Array.isArray(button.sessionGroups) ? button.sessionGroups.filter((group) => typeof group === "string" && group.trim()) : [],
      sendMode: button.sendMode === "raw" ? "raw" : "line",
    })),
    outputRules: (value?.outputRules ?? DEFAULT_SETTINGS.outputRules).filter((rule) => (
      Boolean(rule) && typeof rule.pattern === "string"
    )).map((rule) => ({
      id: rule.id || crypto.randomUUID(),
      name: rule.name?.trim() || rule.pattern,
      enabled: rule.enabled !== false,
      pattern: rule.pattern,
      isRegex: rule.isRegex !== false,
      caseSensitive: rule.caseSensitive === true,
      scope: rule.scope === "hosts" ? "hosts" : "global",
      hosts: Array.isArray(rule.hosts) ? rule.hosts.filter((host) => typeof host === "string" && host.trim()) : [],
      foreground: rule.foreground || undefined,
      background: rule.background || undefined,
      bell: rule.bell === true,
      notify: rule.notify === true,
      autoResponse: rule.autoResponse || undefined,
      cooldownMs: Math.max(0, Number.isFinite(rule.cooldownMs) ? rule.cooldownMs : 1000),
    })),
    autoOpenSftp: value?.autoOpenSftp === true,
    autoShareToCloud: value?.autoShareToCloud === true,
    allowRemoteSend: value?.allowRemoteSend !== false,
    zmodemDownloadPath: value?.zmodemDownloadPath?.trim() || DEFAULT_SETTINGS.zmodemDownloadPath,
    lastSerialConfig: value?.lastSerialConfig ?? DEFAULT_SETTINGS.lastSerialConfig,
    recentSerialConfigs: value?.recentSerialConfigs ?? DEFAULT_SETTINGS.recentSerialConfigs,
    inputHistory: value?.inputHistory ?? DEFAULT_SETTINGS.inputHistory,
    restoreTabsOnStartup: value?.restoreTabsOnStartup ?? DEFAULT_SETTINGS.restoreTabsOnStartup,
    paneLayout: value?.paneLayout ?? DEFAULT_SETTINGS.paneLayout,
    workspaceState: value?.workspaceState ?? DEFAULT_SETTINGS.workspaceState,
    masterPasswordHash: value?.masterPasswordHash ?? DEFAULT_SETTINGS.masterPasswordHash,
    masterPasswordSalt: value?.masterPasswordSalt ?? DEFAULT_SETTINGS.masterPasswordSalt,
    credentialsInitialized: value?.credentialsInitialized ?? DEFAULT_SETTINGS.credentialsInitialized,
    rememberMasterPassword: value?.rememberMasterPassword ?? DEFAULT_SETTINGS.rememberMasterPassword,
    aiConfig: normalizeAiConfig(value?.aiConfig),
    liveRelay: normalizeLiveRelay(value?.liveRelay),
  };
}

export function normalizeLiveRelay(value?: Partial<LiveRelaySettings> | null): LiveRelaySettings {
  const clampCount = (raw: unknown, fallback: number) =>
    typeof raw === "number" && Number.isFinite(raw) ? Math.max(0, Math.floor(raw)) : fallback;
  return {
    enabled: value?.enabled === true,
    allowAttach: value?.allowAttach !== false,
    allowOpenShell: value?.allowOpenShell === true,
    allowOpenSerial: value?.allowOpenSerial === true,
    allowOpenBookmark: value?.allowOpenBookmark === true,
    requireApproval: value?.requireApproval !== false,
    allowRemoteInput: value?.allowRemoteInput !== false,
    maxConcurrentPeers: Math.max(1, clampCount(value?.maxConcurrentPeers, DEFAULT_LIVE_RELAY.maxConcurrentPeers)),
    idleTimeoutMinutes: clampCount(value?.idleTimeoutMinutes, DEFAULT_LIVE_RELAY.idleTimeoutMinutes),
  };
}

export function cloneTerminalTheme(theme: TerminalTheme): TerminalTheme {
  return { ...theme };
}

export function getTerminalThemePreset(id: string) {
  return TERMINAL_THEME_PRESETS.find((preset) => preset.id === id);
}

export function areTerminalThemesEqual(left: TerminalTheme, right: TerminalTheme) {
  return TERMINAL_THEME_KEYS.every((key) => normalizeColor(left[key]) === normalizeColor(right[key]));
}

export function getMatchingTerminalThemePreset(theme: TerminalTheme) {
  return TERMINAL_THEME_PRESETS.find((preset) => areTerminalThemesEqual(preset.theme, theme));
}

interface ParsedColor {
  r: number;
  g: number;
  b: number;
}

function clampChannel(value: number) {
  return Math.max(0, Math.min(255, Math.round(value)));
}

function parseHexColor(value: string) {
  const trimmed = value.trim();
  if (!trimmed.startsWith("#")) {
    return null;
  }

  const hex = trimmed.slice(1);
  if (hex.length === 3) {
    return {
      r: Number.parseInt(hex[0] + hex[0], 16),
      g: Number.parseInt(hex[1] + hex[1], 16),
      b: Number.parseInt(hex[2] + hex[2], 16),
    };
  }

  if (hex.length === 6 || hex.length === 8) {
    return {
      r: Number.parseInt(hex.slice(0, 2), 16),
      g: Number.parseInt(hex.slice(2, 4), 16),
      b: Number.parseInt(hex.slice(4, 6), 16),
    };
  }

  return null;
}

function parseRgbColor(value: string) {
  const match = value.trim().match(/^rgba?\(([^)]+)\)$/i);
  if (!match) {
    return null;
  }

  const parts = match[1].split(",").map((part) => part.trim());
  if (parts.length < 3) {
    return null;
  }

  const [r, g, b] = parts.slice(0, 3).map((part) => Number.parseFloat(part));
  if ([r, g, b].some((part) => Number.isNaN(part))) {
    return null;
  }

  return {
    r: clampChannel(r),
    g: clampChannel(g),
    b: clampChannel(b),
  };
}

function parseColor(value: string, fallback: string): ParsedColor {
  return parseHexColor(value) ?? parseRgbColor(value) ?? parseHexColor(fallback) ?? { r: 0, g: 0, b: 0 };
}

function mixColors(left: ParsedColor, right: ParsedColor, amount: number): ParsedColor {
  const t = Math.max(0, Math.min(1, amount));
  return {
    r: clampChannel(left.r + (right.r - left.r) * t),
    g: clampChannel(left.g + (right.g - left.g) * t),
    b: clampChannel(left.b + (right.b - left.b) * t),
  };
}

function colorToCss(color: ParsedColor) {
  return `rgb(${color.r}, ${color.g}, ${color.b})`;
}

function colorToRgba(color: ParsedColor, alpha: number) {
  return `rgba(${color.r}, ${color.g}, ${color.b}, ${Math.max(0, Math.min(1, alpha))})`;
}

function relativeLuminance(color: ParsedColor) {
  const normalize = (channel: number) => {
    const value = channel / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };

  const r = normalize(color.r);
  const g = normalize(color.g);
  const b = normalize(color.b);
  return (0.2126 * r) + (0.7152 * g) + (0.0722 * b);
}

export function getTerminalThemeAppearance(theme: TerminalTheme): ThemeAppearance {
  const background = parseColor(theme.background, DEFAULT_THEME.background);
  return relativeLuminance(background) >= 0.45 ? "light" : "dark";
}

function resolveUiThemeSource(theme: TerminalTheme, uiThemeMode: UiThemeMode) {
  if (uiThemeMode === "light") {
    return UI_LIGHT_THEME;
  }
  if (uiThemeMode === "dark") {
    return UI_DARK_THEME;
  }
  return theme;
}

export function resolveUiThemeAppearance(theme: TerminalTheme, uiThemeMode: UiThemeMode): ThemeAppearance {
  if (uiThemeMode === "light" || uiThemeMode === "dark") {
    return uiThemeMode;
  }
  return getTerminalThemeAppearance(theme);
}

export function deriveUiTheme(theme: TerminalTheme, uiThemeMode: UiThemeMode = DEFAULT_SETTINGS.uiThemeMode): DerivedUiTheme {
  const resolvedTheme = resolveUiThemeSource(theme, uiThemeMode);
  const appearance = resolveUiThemeAppearance(theme, uiThemeMode);
  const background = parseColor(resolvedTheme.background, DEFAULT_THEME.background);
  const foreground = parseColor(resolvedTheme.foreground, DEFAULT_THEME.foreground);
  const accent = parseColor(
    appearance === "light" ? resolvedTheme.blue : resolvedTheme.brightBlue,
    DEFAULT_THEME.brightBlue,
  );
  const accentAlt = parseColor(resolvedTheme.cyan, DEFAULT_THEME.cyan);
  const danger = parseColor(
    appearance === "light" ? resolvedTheme.red : resolvedTheme.brightRed,
    DEFAULT_THEME.brightRed,
  );
  const success = parseColor(
    appearance === "light" ? resolvedTheme.green : resolvedTheme.brightGreen,
    DEFAULT_THEME.brightGreen,
  );
  const warning = parseColor(
    appearance === "light" ? resolvedTheme.yellow : resolvedTheme.brightYellow,
    DEFAULT_THEME.brightYellow,
  );
  const selection = parseColor(resolvedTheme.selectionBackground, DEFAULT_THEME.selectionBackground);
  const white = { r: 255, g: 255, b: 255 };
  const black = { r: 0, g: 0, b: 0 };

  const surface0 = mixColors(background, foreground, appearance === "light" ? 0.02 : 0.04);
  const surface1 = mixColors(background, foreground, appearance === "light" ? 0.05 : 0.07);
  const surface2 = mixColors(background, foreground, appearance === "light" ? 0.08 : 0.1);
  const surface3 = mixColors(background, foreground, appearance === "light" ? 0.12 : 0.14);
  const surface4 = mixColors(background, foreground, appearance === "light" ? 0.18 : 0.2);
  const border = mixColors(foreground, background, appearance === "light" ? 0.72 : 0.78);
  const borderStrong = mixColors(foreground, background, appearance === "light" ? 0.56 : 0.62);
  const textSecondary = mixColors(foreground, background, 0.34);
  const textMuted = mixColors(foreground, background, 0.5);
  const textDim = mixColors(foreground, background, 0.64);
  const accentHover = mixColors(accent, appearance === "light" ? foreground : white, appearance === "light" ? 0.12 : 0.1);
  const accentContrast = appearance === "light" ? white : parseColor(resolvedTheme.background, DEFAULT_THEME.background);
  const successHover = mixColors(success, appearance === "light" ? foreground : white, appearance === "light" ? 0.08 : 0.12);
  const dangerHover = mixColors(danger, appearance === "light" ? foreground : white, appearance === "light" ? 0.08 : 0.1);
  const terminalPanel = mixColors(background, black, appearance === "light" ? 0.04 : 0.24);
  const terminalPanelAlt = mixColors(background, black, appearance === "light" ? 0.1 : 0.42);
  const shadowBase = appearance === "light" ? foreground : black;

  return {
    appearance,
    variables: {
      "--app-bg": colorToCss(background),
      "--app-surface-0": colorToCss(surface0),
      "--app-surface-1": colorToCss(surface1),
      "--app-surface-2": colorToCss(surface2),
      "--app-surface-3": colorToCss(surface3),
      "--app-surface-4": colorToCss(surface4),
      "--app-panel-bg": colorToCss(surface1),
      "--app-panel-bg-strong": colorToCss(surface2),
      "--app-panel-bg-subtle": colorToCss(surface0),
      "--app-input-bg": colorToCss(surface0),
      "--app-input-bg-focus": colorToCss(surface1),
      "--app-overlay": colorToRgba(shadowBase, appearance === "light" ? 0.2 : 0.56),
      "--app-overlay-soft": colorToRgba(shadowBase, appearance === "light" ? 0.1 : 0.32),
      "--app-shadow": colorToRgba(shadowBase, appearance === "light" ? 0.12 : 0.38),
      "--app-shadow-strong": colorToRgba(shadowBase, appearance === "light" ? 0.2 : 0.58),
      "--app-border": colorToCss(border),
      "--app-border-strong": colorToCss(borderStrong),
      "--app-border-accent": colorToCss(accent),
      "--app-text": colorToCss(foreground),
      "--app-text-secondary": colorToCss(textSecondary),
      "--app-text-muted": colorToCss(textMuted),
      "--app-text-dim": colorToCss(textDim),
      "--app-accent": colorToCss(accent),
      "--app-accent-hover": colorToCss(accentHover),
      "--app-accent-soft": colorToRgba(accent, appearance === "light" ? 0.12 : 0.16),
      "--app-accent-soft-strong": colorToRgba(accent, appearance === "light" ? 0.18 : 0.24),
      "--app-accent-contrast": colorToCss(accentContrast),
      "--app-alt-accent": colorToCss(accentAlt),
      "--app-success": colorToCss(success),
      "--app-success-hover": colorToCss(successHover),
      "--app-success-soft": colorToRgba(success, appearance === "light" ? 0.12 : 0.18),
      "--app-warning": colorToCss(warning),
      "--app-warning-soft": colorToRgba(warning, appearance === "light" ? 0.13 : 0.18),
      "--app-danger": colorToCss(danger),
      "--app-danger-hover": colorToCss(dangerHover),
      "--app-danger-soft": colorToRgba(danger, appearance === "light" ? 0.12 : 0.16),
      "--app-danger-soft-strong": colorToRgba(danger, appearance === "light" ? 0.18 : 0.24),
      "--app-selection": colorToRgba(selection, appearance === "light" ? 0.2 : 0.24),
      "--app-hover": colorToRgba(foreground, appearance === "light" ? 0.06 : 0.08),
      "--app-hover-soft": colorToRgba(foreground, appearance === "light" ? 0.035 : 0.05),
      "--app-scrollbar-track": colorToCss(surface0),
      "--app-scrollbar-thumb": colorToCss(border),
      "--app-scrollbar-thumb-hover": colorToCss(borderStrong),
      "--app-titlebar-bg": colorToCss(surface1),
      "--app-titlebar-subtle": colorToCss(surface0),
      "--app-tabbar-bg": colorToCss(surface0),
      "--app-tab-bg": colorToCss(surface2),
      "--app-tab-bg-hover": colorToCss(surface3),
      "--app-tab-bg-active": colorToCss(surface0),
      "--app-sidebar-bg": colorToCss(surface1),
      "--app-menu-bg": colorToCss(surface1),
      "--app-menu-hover": colorToRgba(accent, appearance === "light" ? 0.14 : 0.22),
      "--app-menu-separator": colorToCss(border),
      "--app-terminal-panel": colorToCss(terminalPanel),
      "--app-terminal-panel-alt": colorToCss(terminalPanelAlt),
      "--app-terminal-gradient": `linear-gradient(180deg, ${colorToCss(surface1)} 0%, ${colorToCss(surface0)} 100%)`,
      "--app-terminal-glow": `radial-gradient(circle at top left, ${colorToRgba(accent, appearance === "light" ? 0.12 : 0.18)} 0%, transparent 26%)`,
      "--app-status-pill-bg": colorToRgba(accent, appearance === "light" ? 0.14 : 0.16),
      "--app-status-pill-fg": colorToCss(accent),
      "--app-dialog-bg": colorToCss(surface1),
      "--app-dialog-bg-secondary": colorToCss(surface2),
      "--app-dialog-fg": colorToCss(foreground),
      "--app-dialog-fg-secondary": colorToCss(textSecondary),
      "--app-dialog-fg-tertiary": colorToCss(textMuted),
      "--bg-dialog": colorToCss(surface1),
      "--fg-dialog": colorToCss(foreground),
      "--bg-secondary": colorToCss(surface2),
      "--border-color": colorToCss(border),
      "--hover-bg": colorToRgba(foreground, appearance === "light" ? 0.06 : 0.08),
      "--btn-secondary-bg": colorToCss(surface2),
      "--btn-secondary-hover": colorToCss(surface3),
      "--btn-secondary-fg": colorToCss(foreground),
      "--btn-border": colorToCss(borderStrong),
      "--btn-primary-bg": colorToCss(accent),
      "--btn-primary-hover": colorToCss(accentHover),
      "--btn-primary-border": colorToCss(accent),
      "--btn-primary-fg": colorToCss(accentContrast),
      "--fg-secondary": colorToCss(textSecondary),
      "--fg-tertiary": colorToCss(textMuted),
      "--accent": colorToCss(accent),
      "--bg-hover": colorToCss(surface3),
    },
  };
}
