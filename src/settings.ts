export interface TerminalTheme {
  background: string;
  foreground: string;
  cursor: string;
}

export interface QuickButton {
  id: string;
  /** 显示名称，为空时展示 command 的前 20 个字符 */
  label: string;
  /** 点击后发送到终端的命令，会自动补 \n */
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

export interface AppSettings {
  fontSize: number;
  fontFamily: string;
  scrollback: number;
  shellPath: string | null;
  theme: TerminalTheme;
  /** 选中即复制：选中文本后自动写入剪贴板；同时 Ctrl+C 在有选中时消费按键（不发 ^C 给 PTY） */
  ctrlCCopy: boolean;
  /** Ctrl+V：粘贴剪贴板内容到终端 */
  ctrlVPaste: boolean;
  /** 鼠标中键：粘贴剪贴板内容到终端 */
  middleClickPaste: boolean;
  /** 终端下方快捷按钮列表 */
  quickButtons: QuickButton[];
  /** 最近一次使用的串口参数 */
  lastSerialConfig: SerialHistoryItem | null;
  /** 最近使用过的串口参数历史，用于快速预设 */
  recentSerialConfigs: SerialHistoryItem[];
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

export const DEFAULT_SETTINGS: AppSettings = {
  fontSize: 15,
  fontFamily: 'Consolas, "Courier New", monospace',
  scrollback: 10000,
  shellPath: null,
  theme: DEFAULT_THEME,
  ctrlCCopy: true,
  ctrlVPaste: true,
  middleClickPaste: true,
  quickButtons: [],
  lastSerialConfig: null,
  recentSerialConfigs: [],
};

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
    theme: nextTheme,
    quickButtons: value?.quickButtons ?? DEFAULT_SETTINGS.quickButtons,
    lastSerialConfig: value?.lastSerialConfig ?? DEFAULT_SETTINGS.lastSerialConfig,
    recentSerialConfigs: value?.recentSerialConfigs ?? DEFAULT_SETTINGS.recentSerialConfigs,
  };
}
