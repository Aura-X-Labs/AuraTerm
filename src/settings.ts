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
  /** 选中即复制：选中文本后自动写入剪贴板；同时 Ctrl+C 在有选中时消费按键（不发 ^C 给 PTY） */
  ctrlCCopy: boolean;
  /** Ctrl+V：粘贴剪贴板内容到终端 */
  ctrlVPaste: boolean;
  /** 鼠标中键：粘贴剪贴板内容到终端 */
  middleClickPaste: boolean;
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
  ctrlCCopy: true,
  ctrlVPaste: true,
  middleClickPaste: true,
};
